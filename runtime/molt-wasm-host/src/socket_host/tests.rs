use super::*;

fn socket_fixture() -> (Store<HostState>, Instance, Memory) {
    let engine = build_engine().unwrap();
    let mut store = Store::new(&engine, main_tests::test_host_state());
    let mut linker = Linker::new(&engine);
    define_socket_host(&mut linker, &mut store).unwrap();
    let module = Module::new(&engine, r#"(module
      (import "env" "molt_socket_getsockopt_host" (func $get (param i64 i32 i32 i32 i32 i32) (result i32)))
      (import "env" "molt_socket_close_host" (func $close (param i64) (result i32)))
      (import "env" "molt_socket_getaddrinfo_host" (func $resolve (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (export "resolve" (func $resolve))
      (memory (export "memory") 1)
      (export "get_option" (func $get))
      (func (export "error") (param i64) (result i32)
        local.get 0 i32.const 1 i32.const 4 i32.const 64 i32.const 4 i32.const 128 call $get)
      (func (export "keepalive") (param i64) (result i32)
        local.get 0 i32.const 1 i32.const 9 i32.const 64 i32.const 4 i32.const 128 call $get)
      (func (export "invalid") (result i32) i64.const -1 call $close))"#).unwrap();
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let memory = instance.get_memory(&mut store, "memory").unwrap();
    store.data_mut().memory = Some(memory);
    (store, instance, memory)
}

#[test]
fn socket_host_option_coordinate_and_error_payload_execute_actual_imports() {
    let (mut store, instance, memory) = socket_fixture();
    let invalid = instance
        .get_typed_func::<(), i32>(&mut store, "invalid")
        .unwrap();
    assert_eq!(invalid.call(&mut store, ()).unwrap(), -9);
    let socket = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
    let handle = store.data_mut().socket_manager.insert(socket);
    let keepalive = instance
        .get_typed_func::<i64, i32>(&mut store, "keepalive")
        .unwrap();
    assert_eq!(keepalive.call(&mut store, handle as i64).unwrap(), 0);
    let mut length = [0; 4];
    memory.read(&store, 128, &mut length).unwrap();
    assert_eq!(u32::from_le_bytes(length), 4);
    let get = instance
        .get_typed_func::<(i64, i32, i32, i32, i32, i32), i32>(&mut store, "get_option")
        .unwrap();
    let bad_length_pointer = memory.data_size(&store) as i32 - 1;
    for option in [4, 13, 9] {
        // SO_ERROR, SO_LINGER, SO_KEEPALIVE independent guest coordinates.
        assert_eq!(
            get.call(
                &mut store,
                (handle as i64, 1, option, 64, 8, bad_length_pointer)
            )
            .unwrap(),
            -14
        );
    }
    assert_eq!(
        get.call(&mut store, (handle as i64, 1, 4, 64, -1, 128))
            .unwrap(),
        -22
    );
    for capacity in [1, 4] {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        socket2::SockRef::from(&server)
            .set_linger(Some(Duration::ZERO))
            .unwrap();
        drop(server);
        let socket = Socket::from(client);
        socket.set_nonblocking(true).unwrap();
        poll_socket(&socket, 1, 1000).unwrap();
        let handle = store.data_mut().socket_manager.insert(socket);
        memory.write(&mut store, 64, &[0xa5; 4]).unwrap();
        let result = if capacity == 4 {
            instance
                .get_typed_func::<i64, i32>(&mut store, "error")
                .unwrap()
                .call(&mut store, handle as i64)
                .unwrap()
        } else {
            get.call(&mut store, (handle as i64, 1, 4, 64, capacity, 128))
                .unwrap()
        };
        assert_eq!(result, 0);
        let mut length = [0; 4];
        memory.read(&store, 128, &mut length).unwrap();
        assert_eq!(u32::from_le_bytes(length), capacity as u32);
        let mut payload = [0; 4];
        memory.read(&store, 64, &mut payload).unwrap();
        if capacity == 1 {
            assert_eq!(payload, [104, 0xa5, 0xa5, 0xa5]);
        } else {
            assert_eq!(i32::from_le_bytes(payload), 104);
        }
        drop(listener);
        let refused = std::net::TcpStream::connect(address).unwrap_err();
        assert_eq!(map_io_error(&refused).code(), 111);
    }
}

#[test]
fn socket_host_input_plans_translate_and_reject_remaining_bits() {
    assert!(native_family(guest_socket_abi::AF_INET6).is_ok());
    assert!(native_socket_option(1, 4).is_ok());
    assert_eq!(
        native_message_flags(i32::MAX, MessageDirection::Receive),
        Err(GuestErrno::EOPNOTSUPP)
    );
    assert!(native_socket_option(1, i32::MAX).is_err());
    assert_eq!(
        native_resolver_arguments(0, 0, 0, i32::MAX),
        Err(GuestResolverError::EAI_BADFLAGS)
    );
    let recv =
        native_message_flags(guest_socket_abi::MSG_DONTWAIT, MessageDirection::Receive).unwrap();
    #[cfg(unix)]
    assert_eq!(recv, libc::MSG_DONTWAIT);
    #[cfg(windows)]
    assert_eq!(recv, 0);
}

#[cfg(unix)]
#[test]
fn socket_host_message_result_projection_keeps_output_bits_separate() {
    assert_eq!(guest_message_result_flags(libc::MSG_TRUNC).unwrap(), 32);
    assert_eq!(guest_message_result_flags(libc::MSG_CTRUNC).unwrap(), 8);
    assert_eq!(guest_message_result_flags(libc::MSG_EOR).unwrap(), 128);
    assert_eq!(
        guest_message_result_flags(libc::MSG_TRUNC | libc::MSG_CTRUNC).unwrap(),
        40
    );
    assert_eq!(
        guest_message_result_flags(i32::MIN),
        Err(GuestErrno::EOPNOTSUPP)
    );
    assert_eq!(
        native_message_flags(guest_socket_abi::MSG_TRUNC, MessageDirection::Receive),
        Err(GuestErrno::EOPNOTSUPP)
    );
}

#[test]
fn socket_host_resolver_length_publication_executes_actual_import() {
    let (mut store, instance, memory) = socket_fixture();
    memory.write(&mut store, 512, b"127.0.0.1").unwrap();
    memory.write(&mut store, 544, b"80").unwrap();
    let resolve = instance
        .get_typed_func::<(i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32), i32>(
            &mut store, "resolve",
        )
        .unwrap();
    // Fixed guest family/type/protocol and NUMERICHOST|NUMERICSERV flags.
    for capacity in [0, 512] {
        assert_eq!(
            resolve
                .call(
                    &mut store,
                    (512, 9, 544, 2, 2, 1, 6, 1028, 1024, capacity, 65535)
                )
                .unwrap(),
            -14
        );
    }
    assert_eq!(
        resolve
            .call(&mut store, (512, 9, 544, 2, 2, 1, 6, 1028, 1024, 0, 640))
            .unwrap(),
        -12
    );
    let mut required = [0; 4];
    memory.read(&store, 640, &mut required).unwrap();
    assert!(u32::from_le_bytes(required) > 0);
    assert_eq!(
        resolve
            .call(&mut store, (512, 9, 544, 2, 2, 1, 6, 1028, 1024, 512, 640))
            .unwrap(),
        0
    );
    assert_eq!(
        resolve
            .call(
                &mut store,
                (512, 9, 544, 2, 2, 1, i32::MAX, 1028, 1024, 512, 640)
            )
            .unwrap(),
        9
    );
}
