use super::*;

#[cfg(unix)]
#[test]
fn native_stat_optional_integers_follow_c_long_semantics() {
    // CPython 3.12-3.14 posixmodule.c calls PyLong_FromLong((long)field).
    // Literal -1/all-bits-set vectors distinguish signed C-long conversion
    // from preserving an unsigned native field or truncating it to u32.
    let mut raw: libc::stat = unsafe { std::mem::zeroed() };
    raw.st_blksize = !0;
    raw.st_blocks = !0;
    let metadata = PathMetadata::from_unix_stat(&raw);
    assert_eq!(metadata.block_size, -1);
    assert_eq!(metadata.blocks, -1);
    if std::mem::size_of_val(&raw.st_blocks) > 4 {
        // Positive in the native field, negative after an ILP32 C-long cast.
        raw.st_blocks = 2_147_483_648_i64 as _;
        let expected = if std::mem::size_of::<libc::c_long>() == 4 {
            -2_147_483_648
        } else {
            2_147_483_648
        };
        assert_eq!(PathMetadata::from_unix_stat(&raw).blocks, expected);
    }
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        raw.st_flags = !0;
        raw.st_gen = !0;
        let metadata = PathMetadata::from_unix_stat(&raw);
        let signed_long_bytes = std::mem::size_of::<libc::c_long>();
        let expected_flags = if std::mem::size_of_val(&raw.st_flags) < signed_long_bytes {
            4_294_967_295
        } else {
            -1
        };
        let expected_generation = if std::mem::size_of_val(&raw.st_gen) < signed_long_bytes {
            4_294_967_295
        } else {
            -1
        };
        assert_eq!(metadata.flags, expected_flags);
        assert_eq!(metadata.generation, expected_generation);
    }
}

#[cfg(any(unix, target_os = "wasi"))]
#[test]
fn stat_device_conventions_preserve_versioned_signedness_and_nodev() {
    use DeviceNumberConvention::{CLong, PythonDev};
    for convention in [CLong, PythonDev] {
        assert_eq!(device_number_integer(0, convention), 0);
        assert_eq!(device_number_integer(42, convention), 42);
    }
    assert_eq!(device_number_integer(u64::MAX - 1, CLong), -2);
    assert_eq!(
        device_number_integer(u64::MAX - 1, PythonDev),
        18_446_744_073_709_551_614
    );
    assert_eq!(device_number_integer(u64::MAX, CLong), -1);
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        // Native NODEV is signed on Apple/OpenBSD and unsigned on FreeBSD.
        #[allow(clippy::unnecessary_cast)]
        let nodev = (!0 as libc::dev_t) as u64;
        let expected = if nodev == 4_294_967_295 {
            4_294_967_295
        } else {
            -1
        };
        assert_eq!(device_number_integer(nodev, PythonDev), expected);
        let mut raw: libc::stat = unsafe { std::mem::zeroed() };
        raw.st_dev = !0;
        assert_eq!(
            PathMetadata::from_unix_stat(&raw).fields[ST_DEV as usize],
            expected
        );
    }
    #[cfg(not(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    assert_eq!(
        device_number_integer(u64::MAX, PythonDev),
        18_446_744_073_709_551_615
    );
}

#[test]
fn exact_time_views_keep_fractional_negative_and_large_native_values() {
    let time = StatTime::from_parts(-1, 999_999_999);
    assert_eq!(time.nanoseconds(), -1);
    assert_eq!(time.seconds(), -1);
    assert!((time.as_float() + 1e-9).abs() < 1e-15);
    let large = StatTime::from_parts(i64::MAX as i128, 999_999_999);
    assert_eq!(
        large.nanoseconds(),
        i64::MAX as i128 * 1_000_000_000 + 999_999_999
    );
    let virtual_max = PathMetadata::from_virtual(true, false, false, 1, u64::MAX);
    assert_eq!(virtual_max.times[1].seconds(), u64::MAX as i128);
    assert_eq!(virtual_max.times[0], StatTime::default());
    // Windows SystemTime counts 100 ns ticks, so a 1 ns step before the epoch
    // is the epoch itself there; 100 ns is exact on every host.
    let before = std::time::UNIX_EPOCH - std::time::Duration::from_nanos(100);
    assert_eq!(
        StatTime::from_system_time(Ok(before)),
        StatTime::from_parts(-1, 999_999_900)
    );
}

#[cfg(windows)]
#[test]
fn windows_metadata_retains_actual_tags_follow_intent_and_shared_handles() {
    use std::os::windows::fs::{OpenOptionsExt, symlink_file};
    let root = std::env::temp_dir().join(format!(
        "molt-stat-record-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let value = root.join("value");
    std::fs::write(&value, b"record").unwrap();
    let held = std::fs::File::open(&value).unwrap();
    // A metadata open must coexist with a reader. Reusing final_path_name's
    // exclusive share=0 contract would fail this operation.
    let query = crate::windows_abi::open_metadata_file(&value, true).unwrap();
    let regular = PathMetadata::from_windows_file(&query).unwrap();
    assert_eq!(regular.fields[ST_SIZE as usize], 6);
    assert_eq!(regular.reparse_tag, 0);
    assert_eq!(crate::windows_abi::file_attribute_tag(&query).unwrap().1, 0);
    assert_eq!(
        PathMetadata::from_windows_file(&held).unwrap().fields,
        regular.fields
    );
    assert_eq!(held.metadata().unwrap().len(), 6); // borrowed, still live
    drop(query);
    drop(held);
    // The path-query owner has retired its handle on success.
    let exclusive = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&value)
        .unwrap();
    drop(exclusive);

    let link = root.join("link");
    symlink_file(&value, &link).unwrap();
    let link_record = PathMetadata::from_windows_path(&link, false).unwrap();
    assert_ne!(
        link_record.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT as u32,
        0
    );
    assert_eq!(link_record.reparse_tag, IO_REPARSE_TAG_SYMLINK as u32);
    let target = PathMetadata::from_windows_path(&link, true).unwrap();
    assert_eq!(target.reparse_tag, 0);
    assert_eq!(target.fields, regular.fields);
    let link_handle = crate::windows_abi::open_metadata_file(&link, false).unwrap();
    assert_eq!(
        PathMetadata::from_windows_file(&link_handle)
            .unwrap()
            .reparse_tag,
        link_record.reparse_tag
    );
    drop(link_handle);
    std::fs::remove_file(&link).unwrap();
    assert_eq!(
        PathMetadata::from_windows_path(&link, false)
            .unwrap_err()
            .raw_os_error(),
        Some(2)
    );

    let directory = root.join("directory");
    std::fs::create_dir(&directory).unwrap();
    let junction = root.join("junction");
    let created = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&directory)
        .output()
        .unwrap();
    assert!(created.status.success(), "junction fixture: {:?}", created);
    let junction_record = PathMetadata::from_windows_path(&junction, false).unwrap();
    assert_eq!(
        junction_record.reparse_tag,
        IO_REPARSE_TAG_MOUNT_POINT as u32
    );
    assert_ne!(junction_record.reparse_tag, link_record.reparse_tag);
    assert_eq!(
        PathMetadata::from_windows_path(&junction, true)
            .unwrap()
            .reparse_tag,
        0
    );
    std::fs::remove_dir(&junction).unwrap();
    assert!(directory.is_dir());
}
