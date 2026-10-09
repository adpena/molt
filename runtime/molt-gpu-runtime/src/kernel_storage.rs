//! One immutable, original-parameter-order physical plan for kernel dispatch.
//! Tensor conversion policies remain with their separate owner. Python kernel
//! values enter this plan only after callback-capable extraction and the current
//! method/default/binding checks; no device path rereads guest storage.
use super::*;

pub(super) struct PhysicalKernelGroup {
    pub name: String,
    pub bytes: Vec<u8>,
    pub hosts: Vec<String>,
    pub writable: bool,
    pub original: Vec<u8>,
    pub format: Option<ScalarFormat>,
    pub elements: usize,
    pub store_flag: Option<usize>,
    destination: Option<u64>,
    rebuilt: Vec<u8>,
}

pub(super) struct KernelStoragePlan {
    pub groups: Vec<PhysicalKernelGroup>,
    pub bindings: BTreeMap<String, usize>,
    flags: Vec<u8>,
}

/// Unpublished COW destinations are created before final admission. Allocation
/// can run finalizers, so the later metadata refresh is authoritative. A callback
/// changing a previously mutable output into a new COW owner is refused before
/// dispatch rather than allocating after that boundary or silently retrying.
pub(super) struct PreparedKernelOutputs<'py> {
    destinations: BTreeMap<u64, OwnedRuntimeValue<'py>>,
}
impl<'py> PreparedKernelOutputs<'py> {
    pub fn new(
        py: &'py PyToken,
        descriptor: &RuntimeKernelDescriptor,
        args: &BTreeMap<String, RuntimeKernelArg<'py>>,
    ) -> Result<Self, u64> {
        let mut destinations = BTreeMap::new();
        for op in &descriptor.ops {
            if op.kind != "store_index" {
                continue;
            }
            let Some(RuntimeKernelArg::Buffer(buffer)) =
                op.args.first().and_then(|name| args.get(name))
            else {
                continue;
            };
            let owner = MoltObject::from_ptr(buffer.object_ptr).bits();
            if destinations.contains_key(&owner) {
                continue;
            }
            let data = obj_from_bits(buffer.data.bits()).as_ptr().ok_or_else(|| {
                raise_exception::<u64>(py, "RuntimeError", "GPU output lost its storage owner")
            })?;
            if unsafe { object_type_id(data) } == TYPE_ID_BYTEARRAY {
                continue;
            }
            let ptr = alloc_bytearray(py, &[]);
            if ptr.is_null() {
                return Err(MoltObject::none().bits());
            }
            destinations.insert(owner, unsafe {
                OwnedRuntimeValue::from_owned_bits(py, MoltObject::from_ptr(ptr).bits())
            });
        }
        Ok(Self { destinations })
    }
}

#[derive(Default)]
struct NumericObservation {
    magnitude: u64,
    negative: bool,
    zero: bool,
}
impl NumericObservation {
    fn observe(&mut self, value: i32, sign_relevant: bool) {
        self.magnitude = self.magnitude.max(i64::from(value).unsigned_abs());
        if sign_relevant {
            self.negative |= value < 0;
            self.zero |= value == 0;
        }
    }
    fn admits(&self, certificate: &JsonValue) -> bool {
        certificate
            .get("alternatives")
            .and_then(JsonValue::as_array)
            .is_some_and(|alternatives| {
                alternatives.iter().any(|alternative| {
                    let Some(bits) = alternative
                        .get("magnitude_bits")
                        .and_then(JsonValue::as_u64)
                        .filter(|bits| *bits <= 24)
                    else {
                        return false;
                    };
                    self.magnitude < (1u64 << bits)
                        && match alternative.get("sign").and_then(JsonValue::as_str) {
                            Some("any") => true,
                            Some("nonnegative") => !self.negative,
                            Some("nonzero") => !self.zero,
                            Some("positive") => !self.negative && !self.zero,
                            _ => false,
                        }
                })
            })
    }
}

fn integral_float(value: f64) -> Result<i32, &'static str> {
    if !value.is_finite()
        || value.fract() != 0.0
        || (value == 0.0 && value.is_sign_negative())
        || value.abs() >= (1u64 << 24) as f64
    {
        return Err("GPU kernel value lacks strict integral precision/signed-zero proof");
    }
    Ok(value as i32)
}
fn integral_integer(value: i64) -> Result<i32, &'static str> {
    if value.unsigned_abs() >= (1u64 << 24) {
        return Err("GPU kernel integer exceeds strict intermediate magnitude capability");
    }
    i32::try_from(value).map_err(|_| "GPU kernel integer exceeds signed shader representation")
}

impl KernelStoragePlan {
    pub fn capture(
        py: &PyToken,
        descriptor: &RuntimeKernelDescriptor,
        args: &BTreeMap<String, RuntimeKernelArg<'_>>,
        prepared: &PreparedKernelOutputs<'_>,
    ) -> Result<Self, u64> {
        let fail = |message: &str| raise_exception::<u64>(py, "RuntimeError", message);
        let reads = descriptor
            .numeric
            .get("read_buffers")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| fail("GPU certificate lacks read storage obligations"))?;
        let reads: BTreeSet<&str> = reads
            .iter()
            .map(|name| {
                name.as_str()
                    .ok_or_else(|| fail("invalid GPU read storage obligation"))
            })
            .collect::<Result<_, _>>()?;
        let writes: BTreeSet<&str> = descriptor
            .ops
            .iter()
            .filter(|op| op.kind == "store_index")
            .filter_map(|op| op.args.first().map(String::as_str))
            .collect();
        let mut groups: Vec<PhysicalKernelGroup> = Vec::new();
        let mut bindings = BTreeMap::new();
        // Mutable bytearray identity is shared; immutable storage is COW for
        // each wrapper. The bool tags distinct object namespaces explicitly.
        let writable_backings: BTreeSet<u64> = writes
            .iter()
            .filter_map(|name| match args.get(*name) {
                Some(RuntimeKernelArg::Buffer(buffer)) => Some(buffer.data.bits()),
                _ => None,
            })
            .collect();
        let mut aliases: BTreeMap<(bool, u64, u8), usize> = BTreeMap::new();
        for name in &descriptor.params {
            let Some(argument) = args.get(name) else {
                continue;
            };
            let index = match argument {
                RuntimeKernelArg::Buffer(buffer) => {
                    let data = obj_from_bits(buffer.data.bits())
                        .as_ptr()
                        .ok_or_else(|| fail("GPU buffer lost its storage owner"))?;
                    let mutable = unsafe { object_type_id(data) } == TYPE_ID_BYTEARRAY;
                    let format =
                        scalar_format_from_text(&buffer.original_format).ok_or_else(|| {
                            fail("GPU buffer format is outside strict kernel capability")
                        })?;
                    // Read-only differing views need not alias on device. If
                    // any view writes, all views must share one element layout.
                    let format_tag = if mutable && !writable_backings.contains(&buffer.data.bits())
                    {
                        match format {
                            ScalarFormat::F32 => 1,
                            ScalarFormat::F64 => 2,
                            ScalarFormat::I64 => 3,
                        }
                    } else {
                        0
                    };
                    let key = if mutable {
                        (true, buffer.data.bits(), format_tag)
                    } else {
                        (false, MoltObject::from_ptr(buffer.object_ptr).bits(), 0)
                    };
                    let writable = writes.contains(name.as_str());
                    if writable && format == ScalarFormat::I64 && buffer.float_elements {
                        return Err(fail(
                            "GPU q store with float conversion would raise in the public struct protocol",
                        ));
                    }
                    if let Some(&index) = aliases.get(&key) {
                        let group = &mut groups[index];
                        // Overlapping views of another width/format cannot
                        // share this element-wise execution representation.
                        if group.format != Some(format) {
                            return Err(fail("GPU shared backing has incompatible element views"));
                        }
                        group.elements = group.elements.max(buffer.size);
                        group.writable |= writable;
                        group.hosts.push(name.clone());
                        index
                    } else {
                        let index = groups.len();
                        aliases.insert(key, index);
                        groups.push(PhysicalKernelGroup {
                            name: name.clone(),
                            bytes: Vec::new(),
                            hosts: vec![name.clone()],
                            writable,
                            original: Vec::new(),
                            format: Some(format),
                            elements: buffer.size,
                            store_flag: None,
                            destination: None,
                            rebuilt: Vec::new(),
                        });
                        index
                    }
                }
                _ => {
                    let index = groups.len();
                    groups.push(PhysicalKernelGroup {
                        name: name.clone(),
                        bytes: Vec::new(),
                        hosts: Vec::new(),
                        writable: false,
                        original: Vec::new(),
                        format: None,
                        elements: 1,
                        store_flag: None,
                        destination: None,
                        rebuilt: Vec::new(),
                    });
                    index
                }
            };
            bindings.insert(name.clone(), index);
        }
        let mut observation = NumericObservation {
            magnitude: descriptor.query_magnitude,
            ..Default::default()
        };
        let mut flag_count: usize = 0;
        for group in &mut groups {
            if let Some(format) = group.format {
                let RuntimeKernelArg::Buffer(buffer) = &args[&group.name] else {
                    unreachable!()
                };
                let view = bytes_like_view(py, buffer.data.bits(), "_data")?;
                let extent = group
                    .elements
                    .checked_mul(format.itemsize())
                    .ok_or_else(|| fail("GPU buffer extent exceeds address space"))?;
                if view.len < extent {
                    return Err(fail(
                        "GPU buffer payload shrank below its final declared extent",
                    ));
                }
                // Snapshot once, after every callback. Encoding and publication
                // use this exact byte generation, including an untouched tail.
                group.original.try_reserve_exact(view.len).map_err(|_| {
                    raise_exception::<u64>(py, "MemoryError", "GPU snapshot allocation failed")
                })?;
                group
                    .original
                    .extend_from_slice(unsafe { std::slice::from_raw_parts(view.ptr, view.len) });
                if group.writable {
                    // Allocate host decode space before the device sees the plan.
                    group
                        .rebuilt
                        .try_reserve_exact(group.original.len())
                        .map_err(|_| {
                            raise_exception::<u64>(
                                py,
                                "MemoryError",
                                "GPU output allocation failed",
                            )
                        })?;
                    group.rebuilt.extend_from_slice(&group.original);
                    let data = obj_from_bits(buffer.data.bits()).as_ptr().unwrap();
                    if unsafe { object_type_id(data) } != TYPE_ID_BYTEARRAY {
                        let owner = MoltObject::from_ptr(buffer.object_ptr).bits();
                        let destination = prepared.destinations.get(&owner).ok_or_else(|| {
                            fail("GPU COW output changed during destination preparation")
                        })?;
                        if !bytearray_copy(destination.bits(), &group.original, false) {
                            return Err(if exception_pending(py) {
                                MoltObject::none().bits()
                            } else {
                                fail("GPU COW destination preparation failed")
                            });
                        }
                        group.destination = Some(destination.bits());
                        group.store_flag = Some(flag_count);
                        flag_count += 1;
                    }
                }
                group
                    .bytes
                    .try_reserve_exact(
                        group
                            .elements
                            .checked_mul(4)
                            .ok_or_else(|| fail("GPU encoded extent exceeds address space"))?
                            .max(4),
                    )
                    .map_err(|_| {
                        raise_exception::<u64>(py, "MemoryError", "GPU encoding allocation failed")
                    })?;
                let sign_relevant = group.hosts.iter().any(|name| reads.contains(name.as_str()));
                for chunk in group.original[..extent].chunks_exact(format.itemsize()) {
                    let value = match format {
                        ScalarFormat::F32 => {
                            integral_float(f64::from(f32::from_ne_bytes(chunk.try_into().unwrap())))
                        }
                        ScalarFormat::F64 => {
                            integral_float(f64::from_ne_bytes(chunk.try_into().unwrap()))
                        }
                        ScalarFormat::I64 => {
                            integral_integer(i64::from_ne_bytes(chunk.try_into().unwrap()))
                        }
                    }
                    .map_err(fail)?;
                    observation.observe(value, sign_relevant);
                    group.bytes.extend_from_slice(&value.to_le_bytes());
                }
                // Storage bindings require a whole element even for an empty
                // logical buffer. Bounds obligations prohibit accessing it.
                if group.bytes.is_empty() {
                    group.bytes.extend_from_slice(&0i32.to_le_bytes());
                }
            } else {
                group.bytes.try_reserve_exact(4).map_err(|_| {
                    raise_exception::<u64>(py, "MemoryError", "GPU scalar allocation failed")
                })?;
                let value = match &args[&group.name] {
                    RuntimeKernelArg::Int(value) => integral_integer(*value),
                    RuntimeKernelArg::Float(value) => integral_float(*value),
                    RuntimeKernelArg::Bool(value) => Ok(i32::from(*value)),
                    RuntimeKernelArg::Buffer(_) => unreachable!(),
                }
                .map_err(fail)?;
                observation.observe(value, true);
                group.bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        if !observation.admits(&descriptor.numeric) {
            return Err(fail(
                "GPU data/geometry does not satisfy the compiler strict numeric certificate",
            ));
        }
        let flag_bytes = flag_count
            .checked_mul(4)
            .ok_or_else(|| fail("GPU store flags exceed address space"))?;
        let mut flags = Vec::new();
        flags.try_reserve_exact(flag_bytes).map_err(|_| {
            raise_exception::<u64>(py, "MemoryError", "GPU store flag allocation failed")
        })?;
        flags.resize(flag_bytes, 0);
        Ok(Self {
            groups,
            bindings,
            flags,
        })
    }

    pub fn binding_count(&self) -> usize {
        self.groups.len() + usize::from(!self.flags.is_empty())
    }

    pub fn bytes(&self, binding: usize) -> &[u8] {
        if binding < self.groups.len() {
            &self.groups[binding].bytes
        } else {
            &self.flags
        }
    }

    pub fn writable(&self, binding: usize) -> bool {
        self.groups.get(binding).is_none_or(|group| group.writable)
    }

    /// Validate and decode the complete readback set before publishing anything.
    /// A store flag records execution, not a byte difference: storing the same
    /// value still performs the public immutable-to-bytearray COW transition.
    pub fn publish(
        &mut self,
        py: &PyToken,
        outputs: &BTreeMap<usize, Vec<u8>>,
        args: &BTreeMap<String, RuntimeKernelArg<'_>>,
    ) -> Result<(), u64> {
        let fail = |message: &str| raise_exception::<u64>(py, "RuntimeError", message);
        let expected = (0..self.binding_count())
            .filter(|&index| self.writable(index))
            .count();
        if outputs.len() != expected {
            return Err(fail("GPU readback binding set differs from dispatch plan"));
        }
        for index in 0..self.binding_count() {
            if !self.writable(index) {
                continue;
            }
            let output = outputs
                .get(&index)
                .ok_or_else(|| fail("GPU readback omitted a writable binding"))?;
            if output.len() != self.bytes(index).len() {
                return Err(fail("GPU readback extent differs from admitted allocation"));
            }
            if index >= self.groups.len()
                && output
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|word| !matches!(u32::from_le_bytes(*word), 0 | 1))
            {
                return Err(fail("GPU store-occurrence flag is invalid"));
            }
        }
        let flag_binding = self.groups.len();
        for group in &mut self.groups {
            if !group.writable {
                continue;
            }
            if group
                .store_flag
                .is_some_and(|flag| outputs[&flag_binding][flag * 4..flag * 4 + 4] == [0, 0, 0, 0])
            {
                continue;
            }
            let format = group.format.expect("only buffer groups are writable");
            let index = self.bindings[&group.name];
            for (element, chunk) in outputs[&index]
                .as_chunks::<4>()
                .0
                .iter()
                .take(group.elements)
                .enumerate()
            {
                let value = i32::from_le_bytes(*chunk);
                // The compiler certificate requires every stored intermediate
                // in the exact float island, independent of transport integrity.
                if i64::from(value).unsigned_abs() >= 1u64 << 24 {
                    return Err(fail("GPU readback violates strict numeric certificate"));
                }
                let offset = element * format.itemsize();
                match format {
                    ScalarFormat::F32 => group.rebuilt[offset..offset + 4]
                        .copy_from_slice(&(value as f32).to_ne_bytes()),
                    ScalarFormat::F64 => group.rebuilt[offset..offset + 8]
                        .copy_from_slice(&f64::from(value).to_ne_bytes()),
                    ScalarFormat::I64 => group.rebuilt[offset..offset + 8]
                        .copy_from_slice(&i64::from(value).to_ne_bytes()),
                }
            }
        }
        // No allocations or Python callbacks follow the first publication.
        // Every old backing, wrapper and destination remains pinned by the
        // enclosing invocation; same-size bytearray mutation preserves exports.
        for group in &self.groups {
            if !group.writable {
                continue;
            }
            if group
                .store_flag
                .is_some_and(|flag| outputs[&flag_binding][flag * 4..flag * 4 + 4] == [0, 0, 0, 0])
            {
                continue;
            }
            let RuntimeKernelArg::Buffer(buffer) = &args[&group.name] else {
                unreachable!()
            };
            let destination = group.destination.unwrap_or(buffer.data.bits());
            if !bytearray_copy(destination, &group.rebuilt, true) {
                return Err(fail("GPU admitted bytearray publication invariant changed"));
            }
            if group.destination.is_some()
                && !commit_buffer_data(
                    MoltObject::from_ptr(buffer.object_ptr).bits(),
                    buffer.data.bits(),
                    destination,
                )
            {
                return Err(fail("GPU admitted COW field publication invariant changed"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kind: &str, args: &[&str], out: Option<&str>, value: Option<i64>) -> RuntimeKernelOp {
        RuntimeKernelOp {
            kind: kind.into(),
            args: args.iter().map(|value| (*value).into()).collect(),
            out: out.map(str::to_owned),
            var: None,
            value,
        }
    }

    fn fixture() -> (RuntimeKernelDescriptor, KernelStoragePlan) {
        let descriptor = RuntimeKernelDescriptor {
            name: "guarded".into(),
            params: vec!["n".into(), "x".into(), "alias".into()],
            ops: vec![
                op("gpu_thread_id", &[], Some("tid"), None),
                op("const", &[], Some("negative"), Some(-1)),
                op("lt", &["tid", "negative"], Some("guard"), None),
                op("if", &["guard"], None, None),
                op("index", &["x", "tid"], Some("old"), None),
                op("store_index", &["alias", "tid", "n"], None, None),
                op("store_index", &["x", "tid", "old"], None, None),
                op("end_if", &[], None, None),
            ],
            code_slot: 0,
            query_bindings: vec![],
            requirements: vec![],
            python_bodies: BTreeMap::new(),
            numeric: JsonValue::Null,
            query_magnitude: 1,
        };
        let plan = KernelStoragePlan {
            groups: vec![
                PhysicalKernelGroup {
                    name: "n".into(),
                    bytes: 7i32.to_le_bytes().to_vec(),
                    hosts: vec![],
                    writable: false,
                    original: vec![],
                    format: None,
                    elements: 1,
                    store_flag: None,
                    destination: None,
                    rebuilt: vec![],
                },
                PhysicalKernelGroup {
                    name: "x".into(),
                    bytes: vec![0; 8],
                    hosts: vec!["x".into(), "alias".into()],
                    writable: true,
                    original: vec![0; 16],
                    format: Some(ScalarFormat::F64),
                    elements: 2,
                    store_flag: Some(0),
                    destination: None,
                    rebuilt: vec![0; 16],
                },
            ],
            bindings: [("n".into(), 0), ("x".into(), 1), ("alias".into(), 1)].into(),
            flags: vec![0; 4],
        };
        (descriptor, plan)
    }

    fn check_shader(dialect: KernelShaderDialect, atomic: &str) {
        let (descriptor, plan) = fixture();
        let shader = render_kernel_source(&descriptor, &plan, 1, 2, dialect).unwrap();
        // Independent selected physical coordinates: scalar is binding0,
        // both buffer aliases use binding1, and occurrence lives in binding2.
        assert!(shader.contains("molt_binding_1[molt_tid] = molt_binding_0[0]"));
        assert!(shader.contains("molt_value_4 = molt_binding_1[molt_tid]"));
        assert!(shader.contains("molt_binding_1[molt_tid] = molt_value_4"));
        assert!(shader.contains("molt_value_2 = molt_tid < -1"));
        assert_eq!(shader.matches(atomic).count(), 1);
        assert_eq!(shader.matches("molt_dirty_0 = true").count(), 2);
        assert!(
            shader.rfind(atomic).unwrap() > shader.rfind("molt_binding_1[molt_tid] =").unwrap()
        );
        assert_eq!(plan.binding_count(), 3);
        assert_eq!(plan.bytes(2), &[0, 0, 0, 0]);
    }

    #[test]
    #[cfg(all(target_os = "macos", feature = "metal-backend"))]
    fn metal_signed_queries_alias_loads_and_one_exit_atomic() {
        check_shader(KernelShaderDialect::Metal, "atomic_store_explicit(");
        let (descriptor, plan) = fixture();
        let shader =
            render_kernel_source(&descriptor, &plan, 1, 2, KernelShaderDialect::Metal).unwrap();
        assert!(shader.contains("const int molt_tid = int(molt_raw_tid)"));
        assert!(shader.contains("memory_order_relaxed"));
    }

    #[test]
    #[cfg(any(target_arch = "wasm32", feature = "webgpu-backend"))]
    fn wgsl_aliases_use_one_binding_and_one_exit_atomic() {
        check_shader(KernelShaderDialect::Wgsl, "atomicStore(");
        let (descriptor, plan) = fixture();
        let shader =
            render_kernel_source(&descriptor, &plan, 1, 2, KernelShaderDialect::Wgsl).unwrap();
        assert_eq!(shader.matches("@binding(").count(), 3);
        assert!(shader.contains("molt_binding_2: array<atomic<u32>>"));
    }

    #[test]
    fn strict_encoding_checks_integer_magnitude_not_significand_count() {
        for value in [
            0.5,
            -0.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            (1u64 << 24) as f64,
            2f64.powi(100),
        ] {
            assert!(integral_float(value).is_err());
        }
        assert_eq!(
            integral_float(-((1u64 << 24) as f64 - 1.0)),
            Ok(-16_777_215)
        );
        assert_eq!(integral_integer(16_777_215), Ok(16_777_215));
        assert!(integral_integer(i64::MIN).is_err());
        assert!(integral_integer(1 << 24).is_err());
    }
}
