//! Execute the production Cranelift call family against an occurrence-counted
//! provider. The input IR already carries the terminal drop plan, so its one
//! explicit retain funds the repeated preboxed transfer; raw boxes belong to
//! the operation transaction under test.
use super::*;
use crate::native_backend::simple_backend::tests::{
    compile_selected_functions_direct, emit_direct_object,
};
use molt_ir::OpKind;
use molt_ir::ParameterCustody::{Borrowed, Transferred};

mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}
mod native_object_execution {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/native_object_execution.rs"
    ));
}

fn fixture(name: &str, kind: OpKind) -> FunctionIR {
    let (arguments, custody, symbol): (Vec<&str>, Vec<_>, Option<&str>) = match kind {
        OpKind::Call | OpKind::CallInternal => (
            vec![
                "wide", "wide", "wide", "wider", "existing", "existing", "other", "wider",
            ],
            vec![
                Borrowed,
                Transferred,
                Transferred,
                Transferred,
                Transferred,
                Transferred,
                Transferred,
                Borrowed,
            ],
            Some("custody_target"),
        ),
        _ => (
            vec!["existing", "wide", "wide", "wider", "existing", "other"],
            vec![Transferred; 6],
            if kind == OpKind::CallGuarded {
                Some("custody_guard_target")
            } else {
                None
            },
        ),
    };
    let function = FunctionIR {
        name: name.into(),
        params: vec!["existing".into(), "other".into()],
        param_types: Some(vec!["dyn".into(), "dyn".into()]),
        parameter_custody: vec![Transferred, Transferred],
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ops: vec![
            OpIR {
                ..OpIR::new(OpKind::DropInserted)
            },
            OpIR {
                out: Some("limb".into()),
                value: Some(1_i64 << 31),
                ..OpIR::new(OpKind::ConstInt)
            },
            OpIR {
                args: Some(vec!["limb".into(), "limb".into()]),
                var: Some("wide".into()),
                out: Some("overflow".into()),
                ..OpIR::new(OpKind::CheckedMul)
            },
            OpIR {
                args: Some(vec!["wide".into(), "limb".into()]),
                var: Some("wider".into()),
                out: Some("overflow2".into()),
                ..OpIR::new(OpKind::CheckedAdd)
            },
            OpIR {
                args: Some(vec!["existing".into()]),
                ..OpIR::new(OpKind::IncRef)
            },
            OpIR {
                args: Some(arguments.into_iter().map(str::to_owned).collect()),
                argument_custody: Some(custody),
                s_value: symbol.map(str::to_owned),
                out: Some("result".into()),
                ..OpIR::new(kind)
            },
            OpIR {
                args: Some(vec!["result".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        ..Default::default()
    };
    let plan = ScalarRepresentationPlan::for_function_ir_for_target(
        &function,
        &crate::tir::TargetInfo::native_release_fast(),
    );
    for operand in ["wide", "wider"] {
        assert!(
            plan.is_full_deopt_int_name(operand),
            "{kind}: fixture must allocate raw operand boxes"
        );
    }
    function
}

fn builder_fixture(name: &str, borrowed_callable: bool) -> FunctionIR {
    let mut function = fixture(name, OpKind::CallFunc);
    function.params = vec!["builder".into()];
    function.param_types = Some(vec!["dyn".into()]);
    function.parameter_custody = vec![Transferred];
    function.ops.retain(|op| op.kind != OpKind::IncRef);
    let call = function
        .ops
        .iter_mut()
        .find(|op| op.kind == OpKind::CallFunc)
        .unwrap();
    call.kind = if borrowed_callable {
        OpKind::CallIndirect
    } else {
        OpKind::CallBind
    };
    call.args = Some(vec!["wide".into(), "builder".into()]);
    call.argument_custody = Some(vec![
        if borrowed_callable {
            Borrowed
        } else {
            Transferred
        },
        Transferred,
    ]);
    function
}

fn loop_fixture() -> FunctionIR {
    let mut function = fixture("custody_loop", OpKind::Call);
    function.params.clear();
    function.param_types = None;
    function.parameter_custody.clear();
    function.ops.truncate(4);
    function.ops.extend([
        OpIR {
            value: Some(10),
            ..OpIR::new(OpKind::Label)
        },
        OpIR {
            s_value: Some("custody_loop_target".into()),
            args: Some(
                ["wide", "wide", "wide", "wider", "wider"]
                    .map(str::to_owned)
                    .to_vec(),
            ),
            argument_custody: Some(vec![
                Borrowed,
                Transferred,
                Transferred,
                Transferred,
                Borrowed,
            ]),
            out: Some("result".into()),
            ..OpIR::new(OpKind::Call)
        },
        OpIR {
            args: Some(vec!["result".into()]),
            ..OpIR::new(OpKind::Release)
        },
        OpIR {
            s_value: Some("custody_loop_continue".into()),
            args: Some(vec![]),
            out: Some("again".into()),
            ..OpIR::new(OpKind::CallInternal)
        },
        OpIR {
            args: Some(vec!["again".into()]),
            value: Some(10),
            ..OpIR::new(OpKind::BrIf)
        },
        OpIR {
            out: Some("none".into()),
            ..OpIR::new(OpKind::ConstNone)
        },
        OpIR {
            args: Some(vec!["none".into()]),
            ..OpIR::new(OpKind::Ret)
        },
    ]);
    function
}

/// A wide raw producer publishes a boxed view in a home. Its authored
/// exception edge is before TRY_END and the next effect, on the production
/// native function compiler path used by the rest of this family fixture.
fn home_store_fixture() -> FunctionIR {
    let mut function = fixture("custody_home_store", OpKind::CallFunc);
    function.params.clear();
    function.param_types = None;
    function.parameter_custody.clear();
    function.return_abi = molt_ir::FunctionReturnAbi::Void;
    function.ops.truncate(3); // drop marker, inline limb, checked wide product
    function.ops.extend([
        OpIR {
            value: Some(51),
            ..OpIR::new(OpKind::TryStart)
        },
        OpIR {
            value: Some(0),
            args: Some(vec!["wide".into()]),
            out: Some("view".into()),
            ..OpIR::new(OpKind::FrameHomeStore)
        },
        OpIR {
            value: Some(51),
            ..OpIR::new(OpKind::CheckException)
        },
        OpIR {
            value: Some(51),
            ..OpIR::new(OpKind::TryEnd)
        },
        OpIR {
            s_value: Some("custody_home_success".into()),
            args: Some(vec!["view".into()]),
            ..OpIR::new(OpKind::Call)
        },
        OpIR {
            value: Some(0),
            ..OpIR::new(OpKind::FrameHomeClear)
        },
        OpIR {
            ..OpIR::new(OpKind::RetVoid)
        },
        OpIR {
            value: Some(51),
            ..OpIR::new(OpKind::Label)
        },
        OpIR {
            value: Some(51),
            ..OpIR::new(OpKind::TryEnd)
        },
        OpIR {
            s_value: Some("custody_home_failure".into()),
            args: Some(vec![]),
            ..OpIR::new(OpKind::Call)
        },
        OpIR {
            value: Some(0),
            ..OpIR::new(OpKind::FrameHomeClear)
        },
        OpIR {
            ..OpIR::new(OpKind::RetVoid)
        },
    ]);
    let plan = ScalarRepresentationPlan::for_function_ir_for_target(
        &function,
        &crate::tir::TargetInfo::native_release_fast(),
    );
    assert!(plan.is_full_deopt_int_name("wide"));
    assert!(
        !plan.is_raw_int_carrier_name("view"),
        "fixture requires persistent boxed publication"
    );
    function
}

fn home_store_alias_fixture() -> FunctionIR {
    let mut function = home_store_fixture();
    function.name = "custody_home_store_alias".into();
    let end = function
        .ops
        .iter()
        .position(|op| op.kind == OpKind::TryEnd)
        .unwrap();
    function.ops.insert(
        end,
        OpIR {
            args: Some(vec!["view".into()]),
            out: Some("view_alias".into()),
            ..OpIR::new(OpKind::IdentityAlias)
        },
    );
    let call = function
        .ops
        .iter_mut()
        .find(|op| op.s_value.as_deref() == Some("custody_home_success"))
        .unwrap();
    call.args = Some(vec!["view_alias".into()]);
    function
}

/// The same wide publication stays allocation-free when only arithmetic
/// observes its view. A branch exposes the independent arithmetic result;
/// the provider also checks that no temporary or persistent box was minted.
fn home_store_numeric_fixture() -> FunctionIR {
    let mut function = home_store_fixture();
    function.name = "custody_home_numeric".into();
    let call = function
        .ops
        .iter()
        .position(|op| op.s_value.as_deref() == Some("custody_home_success"))
        .unwrap();
    function.ops.splice(
        call..=call,
        [
            OpIR {
                args: Some(vec!["view".into(), "limb".into()]),
                var: Some("numeric".into()),
                out: Some("numeric_overflow".into()),
                ..OpIR::new(OpKind::CheckedAdd)
            },
            OpIR {
                value: Some((1_i64 << 62) + (1_i64 << 31)),
                out: Some("numeric_expected".into()),
                ..OpIR::new(OpKind::ConstInt)
            },
            OpIR {
                args: Some(vec!["numeric".into(), "numeric_expected".into()]),
                out: Some("numeric_ok".into()),
                ..OpIR::new(OpKind::Eq)
            },
            OpIR {
                args: Some(vec!["numeric_ok".into()]),
                value: Some(60),
                ..OpIR::new(OpKind::BrIf)
            },
            OpIR {
                s_value: Some("custody_home_numeric_bad".into()),
                args: Some(vec![]),
                ..OpIR::new(OpKind::Call)
            },
            OpIR {
                value: Some(60),
                ..OpIR::new(OpKind::Label)
            },
            OpIR {
                s_value: Some("custody_home_numeric_ok".into()),
                args: Some(vec![]),
                ..OpIR::new(OpKind::Call)
            },
        ],
    );
    let failure = function
        .ops
        .iter_mut()
        .find(|op| op.s_value.as_deref() == Some("custody_home_failure"))
        .unwrap();
    failure.s_value = Some("custody_home_numeric_bad".into());
    let plan = ScalarRepresentationPlan::for_function_ir_for_target(
        &function,
        &crate::tir::TargetInfo::native_release_fast(),
    );
    for value in ["view", "numeric"] {
        assert!(
            plan.is_full_deopt_int_name(value),
            "numeric-only view must remain raw: {value}"
        );
    }
    function
}

fn home_store_bitwise_fixture(observed: bool) -> FunctionIR {
    let mut function = home_store_numeric_fixture();
    function.name = if observed {
        "custody_home_bitwise_observed"
    } else {
        "custody_home_bitwise_raw"
    }
    .into();
    let index = function
        .ops
        .iter()
        .position(|op| op.kind == OpKind::CheckedAdd)
        .unwrap();
    function.ops[index] = OpIR {
        args: Some(vec!["view".into(), "limb".into()]),
        out: Some("numeric".into()),
        ..OpIR::new(OpKind::BitAnd)
    };
    function
        .ops
        .iter_mut()
        .find(|op| op.out.as_deref() == Some("numeric_expected"))
        .unwrap()
        .value = Some(0);
    // Integer `not` must not mint a temporary box in either machine backend.
    // Branching to the failing provider exposes its mathematical result too.
    function.ops.splice(
        index..index,
        [
            OpIR {
                args: Some(vec!["view".into()]),
                out: Some("wide_is_zero".into()),
                ..OpIR::new(OpKind::Not)
            },
            OpIR {
                args: Some(vec!["wide_is_zero".into()]),
                value: Some(61),
                ..OpIR::new(OpKind::BrIf)
            },
        ],
    );
    let failure = function
        .ops
        .iter()
        .position(|op| op.s_value.as_deref() == Some("custody_home_numeric_bad"))
        .unwrap();
    function.ops.insert(
        failure,
        OpIR {
            value: Some(61),
            ..OpIR::new(OpKind::Label)
        },
    );
    if observed {
        let index = function
            .ops
            .iter()
            .position(|op| op.kind == OpKind::BitAnd)
            .unwrap();
        function.ops.insert(
            index,
            OpIR {
                s_value: Some("custody_home_success".into()),
                args: Some(vec!["view".into()]),
                ..OpIR::new(OpKind::Call)
            },
        );
        function
            .ops
            .iter_mut()
            .find(|op| op.s_value.as_deref() == Some("custody_home_numeric_ok"))
            .unwrap()
            .s_value = Some("custody_home_observed_numeric_ok".into());
        function
            .ops
            .iter_mut()
            .rev()
            .find(|op| op.s_value.as_deref() == Some("custody_home_numeric_bad"))
            .unwrap()
            .s_value = Some("custody_home_failure".into());
    }
    let plan = ScalarRepresentationPlan::for_function_ir_for_target(
        &function,
        &crate::tir::TargetInfo::native_release_fast(),
    );
    for value in ["view", "numeric"] {
        assert_eq!(plan.is_raw_int_carrier_name(value), !observed, "{value}");
    }
    function
}

fn method_receiver_prefix_fixture() -> FunctionIR {
    FunctionIR {
        name: "custody_method_receiver".into(),
        params: vec!["builder".into(), "receiver".into()],
        param_types: Some(vec!["dyn".into(), "dyn".into()]),
        parameter_custody: vec![Borrowed, Transferred],
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ops: vec![
            OpIR {
                ..OpIR::new(OpKind::DropInserted)
            },
            OpIR {
                args: Some(vec!["builder".into(), "receiver".into()]),
                ..OpIR::new(OpKind::CallargsPushMethodReceiver)
            },
            OpIR {
                out: Some("nothing".into()),
                ..OpIR::new(OpKind::ConstNone)
            },
            OpIR {
                args: Some(vec!["nothing".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        ..Default::default()
    }
}

#[test]
fn native_call_operands_execute_identity_custody_and_mint_failure() {
    let rustc = native_object_execution::rustc_path();
    let cases = [
        ("custody_direct", OpKind::Call),
        ("custody_internal", OpKind::CallInternal),
        ("custody_guarded", OpKind::CallGuarded),
        ("custody_dynamic", OpKind::CallFunc),
        ("custody_method", OpKind::CallMethod),
        ("custody_loaded_method", OpKind::CallLoadedMethod),
    ];
    let mut functions: Vec<_> = cases
        .iter()
        .map(|(name, kind)| fixture(name, *kind))
        .collect();
    functions.push(builder_fixture("custody_bind", false));
    functions.push(builder_fixture("custody_indirect", true));
    functions.push(loop_fixture());
    functions.push(home_store_fixture());
    functions.push(home_store_alias_fixture());
    functions.push(home_store_numeric_fixture());
    functions.push(home_store_bitwise_fixture(false));
    functions.push(home_store_bitwise_fixture(true));
    functions.push(method_receiver_prefix_fixture());
    for (name, arity, custody) in [
        (
            "custody_target",
            8,
            vec![
                Borrowed,
                Transferred,
                Transferred,
                Transferred,
                Transferred,
                Transferred,
                Transferred,
                Borrowed,
            ],
        ),
        ("custody_guard_target", 5, vec![Transferred; 5]),
        (
            "custody_loop_target",
            5,
            vec![Borrowed, Transferred, Transferred, Transferred, Borrowed],
        ),
        ("custody_loop_continue", 0, vec![]),
        ("custody_home_success", 1, vec![Borrowed]),
        ("custody_home_failure", 0, vec![]),
        ("custody_home_numeric_ok", 0, vec![]),
        ("custody_home_numeric_bad", 0, vec![]),
        ("custody_home_observed_numeric_ok", 0, vec![]),
    ] {
        functions.push(FunctionIR {
            name: name.into(),
            is_extern: true,
            params: (0..arity).map(|i| format!("arg{i}")).collect(),
            param_types: Some(vec!["dyn".into(); arity]),
            parameter_custody: custody,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            ..Default::default()
        });
    }
    let targets: Vec<_> = cases
        .iter()
        .map(|(name, _)| *name)
        .chain([
            "custody_bind",
            "custody_indirect",
            "custody_loop",
            "custody_home_store",
            "custody_home_store_alias",
            "custody_home_numeric",
            "custody_home_bitwise_raw",
            "custody_home_bitwise_observed",
            "custody_method_receiver",
        ])
        .collect();
    let object = emit_direct_object(compile_selected_functions_direct(functions, &targets));
    let provider = PROVIDER
        .replace(
            "@INT_TAG@",
            &(molt_codegen_abi::QNAN_TAG_INT_I64 as u64).to_string(),
        )
        .replace(
            "@NONE@",
            &(molt_codegen_abi::box_none_bits() as u64).to_string(),
        )
        .replace(
            "@TRUE@",
            &(molt_codegen_abi::box_bool_bits(1) as u64).to_string(),
        )
        .replace(
            "@FALSE@",
            &(molt_codegen_abi::box_bool_bits(0) as u64).to_string(),
        )
        .replace(
            "@HOME_UNBOUND@",
            &molt_codegen_abi::FRAME_HOME_UNBOUND.to_string(),
        )
        .replace(
            "@HOME_PLAIN@",
            &molt_codegen_abi::FRAME_HOME_PLAIN.to_string(),
        )
        .replace(
            "@HOME_RAW@",
            &molt_codegen_abi::FRAME_HOME_RAW_INT.to_string(),
        )
        .replace("@ABI@", molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL);
    let provider = provider.replace(
        "@METHOD_NULL@",
        &(molt_codegen_abi::loaded_method_null_bits() as u64).to_string(),
    );
    let harness = HARNESS
        .replace(
            "@NONE@",
            &(molt_codegen_abi::box_none_bits() as u64).to_string(),
        )
        .replace(
            "@METHOD_NULL@",
            &(molt_codegen_abi::loaded_method_null_bits() as u64).to_string(),
        );
    native_object_execution::link_and_run_native_object(
        &rustc,
        "native-call-operand-custody",
        object,
        &provider,
        &harness,
        "native call operand custody",
    );
}

const PROVIDER: &str = r#"#![no_std]
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};
const NONE:u64=@NONE@;
const TRUE:u64=@TRUE@;
const FALSE:u64=@FALSE@;
const METHOD_NULL:u64=@METHOD_NULL@;
#[export_name="@ABI@"] pub static ABI:u8=0;
static mut PENDING:u8=0;
static ATTEMPTS:AtomicU64=AtomicU64::new(0);
static FAIL:AtomicU64=AtomicU64::new(0);
static CALLS:AtomicU64=AtomicU64::new(0);
static ERROR:AtomicU64=AtomicU64::new(0);
static ABORTS:AtomicU64=AtomicU64::new(0);
static MODE:AtomicU64=AtomicU64::new(0);
static GUARD_FAIL:AtomicU64=AtomicU64::new(0);
static FIRST:AtomicU64=AtomicU64::new(0);
static SECOND:AtomicU64=AtomicU64::new(0);
static EXISTING:AtomicU64=AtomicU64::new(0);
static OTHER:AtomicU64=AtomicU64::new(0);
static RESULT:AtomicU64=AtomicU64::new(0);
static PREFIX_PUSHES:AtomicU64=AtomicU64::new(0);
static PREFIX_FAIL:AtomicU64=AtomicU64::new(0);
fn refs(word:u64)->Option<&'static AtomicU64>{match word{0x101=>Some(&FIRST),0x102=>Some(&SECOND),0x501=>Some(&EXISTING),0x502=>Some(&OTHER),0x200=>Some(&RESULT),_=>None}}
#[no_mangle] pub extern "C" fn molt_inc_ref_obj(word:u64){if let Some(refs)=refs(word){assert!(refs.fetch_add(1,SeqCst)>0,"retain after release");}}
#[no_mangle] pub extern "C" fn molt_dec_ref_obj(word:u64){assert_ne!(word,METHOD_NULL,"null receiver crossed RC boundary");if let Some(refs)=refs(word){let old=refs.fetch_sub(1,SeqCst);assert!(old>0,"duplicate release");if old==1&&matches!(word,0x501|0x502)&&ERROR.load(SeqCst)!=0&&PREFIX_FAIL.load(SeqCst)==0{ERROR.store(0xd00d,SeqCst);}}}
#[no_mangle] pub extern "C" fn molt_dec_ref(word:u64){molt_dec_ref_obj(word)}
#[no_mangle] pub extern "C" fn molt_int_from_i64(raw:i64)->u64{
 assert_eq!(ERROR.load(SeqCst),0,"later box after first failure");let attempt=ATTEMPTS.fetch_add(1,SeqCst)+1;
 if FAIL.load(SeqCst)==attempt{ERROR.store(0xb000+attempt,SeqCst);unsafe{PENDING=1;}return NONE;}
 let (word,owner)=if raw==(1_i64<<62){(0x101,&FIRST)}else{assert_eq!(raw,(1_i64<<62)+(1_i64<<31));(0x102,&SECOND)};
 assert_eq!(owner.swap(1,SeqCst),0,"repeated value rematerialized");word
}
#[no_mangle] pub extern "C" fn molt_exception_pending_fast()->u64{u64::from(ERROR.load(SeqCst)!=0)}
#[no_mangle] pub extern "C" fn molt_exception_pending()->u64{molt_exception_pending_fast()}
#[no_mangle] pub extern "C" fn molt_async_work_poll_and_exception_pending()->u64{molt_exception_pending_fast()}
#[no_mangle] pub extern "C" fn molt_thread_record()->u64{core::ptr::addr_of!(PENDING) as u64}
#[no_mangle] pub extern "C" fn molt_is_truthy(word:u64)->u64{assert!(word==TRUE||word==FALSE);u64::from(word==TRUE)}
#[no_mangle] pub extern "C" fn molt_recursion_enter_fast()->u64{1-GUARD_FAIL.load(SeqCst)}
#[no_mangle] pub extern "C" fn molt_recursion_exit_fast(){}
#[no_mangle] pub extern "C" fn molt_raise_recursion_error()->u64{ERROR.store(0xc000,SeqCst);unsafe{PENDING=1;}NONE}
#[no_mangle] pub extern "C" fn molt_function_direct_call_eligible(_:u64,_:u64,_:u64)->u64{0}
#[no_mangle] pub extern "C" fn molt_handle_resolve(_:u64)->u64{panic!("ineligible callable must not be resolved")}
unsafe fn words<'a>(p:u64,n:u64)->&'a[u64]{if n==0{&[]}else{unsafe{core::slice::from_raw_parts(p as *const u64,n as usize)}}}
fn release(words:&[u64]){for &word in words{molt_dec_ref_obj(word);}}
fn result()->u64{CALLS.fetch_add(1,SeqCst);assert_eq!(RESULT.swap(1,SeqCst),0);0x200}
fn enter(first:u64,second:u64){assert_eq!(ERROR.load(SeqCst),0,"consumer after failed mint");assert_eq!(FIRST.load(SeqCst),first);assert_eq!(SECOND.load(SeqCst),second);assert_eq!(EXISTING.load(SeqCst),2);assert_eq!(OTHER.load(SeqCst),1);}
#[no_mangle] pub extern "C" fn custody_target(borrow:u64,a:u64,b:u64,c:u64,existing:u64,again:u64,other:u64,later:u64)->u64{
 assert_eq!((borrow,a,b,c,later),(0x101,0x101,0x101,0x102,0x102));assert_eq!((existing,again,other),(0x501,0x501,0x502));enter(3,2);
 release(&[a,b,c,existing,again,other]);assert_eq!(FIRST.load(SeqCst),1);assert_eq!(SECOND.load(SeqCst),1);result()
}
#[no_mangle] pub extern "C" fn custody_guard_target(_:u64,_:u64,_:u64,_:u64,_:u64)->u64{panic!("fixture forces guarded fallback")}
#[no_mangle] pub extern "C" fn custody_loop_target(borrow:u64,a:u64,b:u64,c:u64,later:u64)->u64{
 assert_eq!((borrow,a,b,c,later),(0x101,0x101,0x101,0x102,0x102));assert_eq!(ERROR.load(SeqCst),0);assert_eq!(FIRST.load(SeqCst),3);assert_eq!(SECOND.load(SeqCst),2);
 release(&[a,b,c]);assert_eq!(FIRST.load(SeqCst),1);assert_eq!(SECOND.load(SeqCst),1);result()
}
#[no_mangle] pub extern "C" fn custody_loop_continue()->u64{
 for count in [&FIRST,&SECOND,&RESULT]{assert_eq!(count.load(SeqCst),0,"previous iteration owner survived");}
 if ERROR.load(SeqCst)==0&&CALLS.load(SeqCst)<2{TRUE}else{FALSE}
}
#[no_mangle] pub extern "C" fn molt_call_func_owned(callable:u64,p:u64,n:u64,_:u64)->u64{
 assert_eq!(callable,0x501);let args=unsafe{words(p,n)};assert_eq!(args,&[0x101,0x101,0x102,0x501,0x502]);enter(2,1);release(args);molt_dec_ref_obj(callable);result()
}
#[no_mangle] pub extern "C" fn molt_call_loaded_method4(callable:u64,self_or_null:u64,a:u64,b:u64,c:u64,d:u64)->u64{
 assert_eq!(callable,0x501);let args=[self_or_null,a,b,c,d];assert_eq!(args,[0x101,0x101,0x102,0x501,0x502]);enter(2,1);release(&args);molt_dec_ref_obj(callable);result()
}
fn bind(callable:u64,builder:u64,owned:bool)->u64{assert_eq!(ERROR.load(SeqCst),0);assert_eq!(callable,0x101);assert_eq!(builder,0x501);assert_eq!(FIRST.load(SeqCst),1);assert_eq!(EXISTING.load(SeqCst),1);molt_dec_ref_obj(builder);if owned{molt_dec_ref_obj(callable);}result()}
#[no_mangle] pub extern "C" fn molt_call_bind_ic_owned(_:u64,c:u64,b:u64)->u64{bind(c,b,true)}
#[no_mangle] pub extern "C" fn molt_call_bind_ic(_:u64,c:u64,b:u64)->u64{bind(c,b,false)}
#[no_mangle] pub extern "C" fn molt_call_indirect_ic(_:u64,c:u64,b:u64)->u64{bind(c,b,false)}
#[no_mangle] pub extern "C" fn molt_call_inputs_release(callable:u64,p:u64,n:u64){
 let args=unsafe{words(p,n)};let error=ERROR.load(SeqCst);assert_ne!(error,0);ABORTS.fetch_add(1,SeqCst);
 if error<0xc000{let mode=MODE.load(SeqCst);if mode==1{assert_eq!(callable,0x501);assert_eq!(args,&[0x501,0x502]);}else if mode==2{assert_eq!(callable,0);assert_eq!(args,&[0x501]);}else{assert_eq!(callable,0);assert_eq!(args,&[0x501,0x501,0x502]);}}
 release(args);molt_dec_ref_obj(callable);ERROR.store(error,SeqCst);
}
#[no_mangle] pub extern "C" fn custody_reset(mode:u64,fail:u64,guard:u64){for value in [&ATTEMPTS,&CALLS,&ERROR,&ABORTS,&FIRST,&SECOND,&RESULT]{value.store(0,SeqCst);}unsafe{PENDING=0;}EXISTING.store(u64::from(mode!=3),SeqCst);OTHER.store(u64::from(mode<2),SeqCst);MODE.store(mode,SeqCst);FAIL.store(fail,SeqCst);GUARD_FAIL.store(guard,SeqCst);}
#[no_mangle] pub extern "C" fn custody_check(fail:u64,boxes:u64,guard:u64){let failed=fail!=0||guard!=0;assert_eq!(CALLS.load(SeqCst),u64::from(!failed));assert_eq!(ATTEMPTS.load(SeqCst),if fail!=0{fail}else{boxes});assert_eq!(ERROR.load(SeqCst),if fail!=0{0xb000+fail}else if guard!=0{0xc000}else{0});assert_eq!(ABORTS.load(SeqCst),u64::from(failed));for count in [&FIRST,&SECOND,&EXISTING,&OTHER,&RESULT]{assert_eq!(count.load(SeqCst),0,"owner escaped operation");}}
#[no_mangle] pub extern "C" fn custody_prefix_reset(present:u64,fail:u64){PREFIX_PUSHES.store(0,SeqCst);PREFIX_FAIL.store(fail,SeqCst);EXISTING.store(present,SeqCst);ERROR.store(0,SeqCst);unsafe{PENDING=0;}}
#[no_mangle] pub extern "C" fn molt_callargs_push_pos(builder:u64,receiver:u64)->u64{
 assert_eq!((builder,receiver),(0x777,0x501));assert_eq!(PREFIX_PUSHES.fetch_add(1,SeqCst),0);assert_eq!(EXISTING.load(SeqCst),1);
 if PREFIX_FAIL.load(SeqCst)!=0{ERROR.store(0xbad,SeqCst);unsafe{PENDING=1;}}else{molt_inc_ref_obj(receiver);}NONE
}
#[no_mangle] pub extern "C" fn custody_prefix_check(present:u64,fail:u64){
 assert_eq!(PREFIX_PUSHES.load(SeqCst),present);assert_eq!(ERROR.load(SeqCst),if present!=0&&fail!=0{0xbad}else{0});
 assert_eq!(EXISTING.load(SeqCst),u64::from(present!=0&&fail==0),"builder is the sole remaining receiver owner");
 if present!=0&&fail==0{molt_dec_ref_obj(0x501);}assert_eq!(EXISTING.load(SeqCst),0);PREFIX_FAIL.store(0,SeqCst);
}
#[no_mangle] pub extern "C" fn custody_loop_check(fail:u64){assert_eq!(CALLS.load(SeqCst),if fail==0{2}else{(fail-1)/2});assert_eq!(ATTEMPTS.load(SeqCst),if fail==0{4}else{fail});assert_eq!(ERROR.load(SeqCst),if fail==0{0}else{0xb000+fail});assert_eq!(ABORTS.load(SeqCst),0);for count in [&FIRST,&SECOND,&EXISTING,&OTHER,&RESULT]{assert_eq!(count.load(SeqCst),0,"loop owner escaped");}}

// Home publication uses the same counted allocator and pending-error provider.
const HOME_UNBOUND: u64 = @HOME_UNBOUND@;
const HOME_PLAIN: u64 = @HOME_PLAIN@;
const HOME_RAW: u64 = @HOME_RAW@;
static mut HOME: [u64; 2] = [HOME_UNBOUND, 0];
static HOME_SUCCESSES: AtomicU64 = AtomicU64::new(0);
static HOME_FAILURES: AtomicU64 = AtomicU64::new(0);
fn home_pair() -> (u64, u64) {
    unsafe { (core::ptr::addr_of!(HOME[0]).read(), core::ptr::addr_of!(HOME[1]).read()) }
}
#[no_mangle]
pub extern "C" fn molt_frame_homes(slots: u64) -> u64 {
    assert_eq!(slots, 1);
    core::ptr::addr_of_mut!(HOME) as u64
}
#[no_mangle]
pub extern "C" fn molt_frame_home_load(home: u64) -> u64 {
    assert_eq!(home, core::ptr::addr_of_mut!(HOME) as u64);
    let (kind, bits) = home_pair();
    if kind == HOME_PLAIN { return bits; }
    assert_eq!((kind, bits), (HOME_RAW, 1_u64 << 62));
    let boxed = molt_int_from_i64(bits as i64);
    if ERROR.load(SeqCst) != 0 {
        assert_eq!(home_pair(), (HOME_RAW, bits));
        return 0x600;
    }
    unsafe { HOME = [HOME_PLAIN, boxed]; }
    boxed
}
#[no_mangle]
pub extern "C" fn custody_home_success(view: u64) -> u64 {
    assert_eq!(ERROR.load(SeqCst), 0, "store failure crossed the authored exception edge");
    assert_eq!(HOME_SUCCESSES.fetch_add(1, SeqCst), 0);
    assert_eq!((view, home_pair()), (0x101, (HOME_PLAIN, 0x101)));
    assert_eq!(FIRST.load(SeqCst), 1, "home owns the persistent box");
    NONE
}
#[no_mangle]
pub extern "C" fn custody_home_failure() -> u64 {
    assert_eq!(ERROR.load(SeqCst), 0xb001, "intended handler observes original allocation failure");
    assert_eq!(HOME_FAILURES.fetch_add(1, SeqCst), 0);
    assert_eq!(HOME_SUCCESSES.load(SeqCst), 0);
    assert_eq!(home_pair(), (HOME_RAW, 1_u64 << 62));
    NONE
}
#[no_mangle]
pub extern "C" fn custody_home_reset(failure: u64) {
    assert_eq!(home_pair(), (HOME_UNBOUND, 0));
    custody_reset(3, failure, 0);
    HOME_SUCCESSES.store(0, SeqCst);
    HOME_FAILURES.store(0, SeqCst);
}
#[no_mangle]
pub extern "C" fn custody_home_finish(failure: u64) {
    assert_eq!(ATTEMPTS.load(SeqCst), 1);
    assert_eq!(ERROR.load(SeqCst), if failure == 0 { 0 } else { 0xb001 });
    assert_eq!(HOME_SUCCESSES.load(SeqCst), u64::from(failure == 0));
    assert_eq!(HOME_FAILURES.load(SeqCst), u64::from(failure != 0));
    assert_eq!(home_pair(), (HOME_UNBOUND, 0));
    for count in [&FIRST, &SECOND, &EXISTING, &OTHER, &RESULT] {
        assert_eq!(count.load(SeqCst), 0, "every home owner released exactly once");
    }
}
#[no_mangle]
pub extern "C" fn custody_home_numeric_ok() -> u64 {
    assert_eq!(ATTEMPTS.load(SeqCst), 0, "numeric-only publication allocated");
    assert_eq!(ERROR.load(SeqCst), 0);
    assert_eq!(HOME_SUCCESSES.fetch_add(1, SeqCst), 0);
    assert_eq!(home_pair(), (HOME_RAW, 1_u64 << 62));
    NONE
}
#[no_mangle]
pub extern "C" fn custody_home_numeric_bad() -> u64 {
    panic!("numeric-only publication changed the arithmetic result or raised");
}
#[no_mangle]
pub extern "C" fn custody_home_observed_numeric_ok() -> u64 {
    assert_eq!(ATTEMPTS.load(SeqCst), 1);
    assert_eq!(HOME_SUCCESSES.load(SeqCst), 1);
    assert_eq!(home_pair(), (HOME_PLAIN, 0x101));
    NONE
}
#[no_mangle]
pub extern "C" fn molt_bit_and(lhs: u64, rhs: u64) -> u64 {
    assert_eq!(lhs, 0x101, "the persistent binding's exact object");
    assert_eq!(rhs, @INT_TAG@ | (1_u64 << 31));
    @INT_TAG@ // (2**62 & 2**31) == 0, as a boxed inline integer.
}
#[no_mangle]
pub extern "C" fn molt_eq(lhs: u64, rhs: u64) -> u64 {
    assert_eq!((lhs, rhs), (@INT_TAG@, @INT_TAG@));
    TRUE
}
#[no_mangle]
pub extern "C" fn molt_not(value: u64) -> u64 {
    assert_eq!(value, 0x101);
    assert_eq!(home_pair(), (HOME_PLAIN, value));
    FALSE
}
#[no_mangle]
pub extern "C" fn custody_home_numeric_finish() {
    assert_eq!(ATTEMPTS.load(SeqCst), 0);
    assert_eq!(ERROR.load(SeqCst), 0);
    assert_eq!(HOME_SUCCESSES.load(SeqCst), 1);
    assert_eq!(HOME_FAILURES.load(SeqCst), 0);
    assert_eq!(home_pair(), (HOME_UNBOUND, 0));
    for count in [&FIRST, &SECOND, &EXISTING, &OTHER, &RESULT] {
        assert_eq!(count.load(SeqCst), 0, "numeric-only publication retained a box");
    }
}
"#;

const HARNESS: &str = r#"
const NONE:u64=@NONE@;
const METHOD_NULL:u64=@METHOD_NULL@;
extern "C" {
 fn custody_direct(a:u64,b:u64)->u64; fn custody_internal(a:u64,b:u64)->u64;
 fn custody_guarded(a:u64,b:u64)->u64; fn custody_dynamic(a:u64,b:u64)->u64;
 fn custody_method(a:u64,b:u64)->u64; fn custody_loaded_method(a:u64,b:u64)->u64;
 fn custody_bind(builder:u64)->u64;
 fn custody_indirect(builder:u64)->u64;
 fn custody_loop()->u64; fn custody_loop_check(fail:u64);
 fn custody_reset(mode:u64,fail:u64,guard:u64); fn custody_check(fail:u64,boxes:u64,guard:u64);
 fn custody_home_store(); fn custody_home_reset(failure:u64); fn custody_home_finish(failure:u64);
 fn custody_home_store_alias(); fn custody_home_numeric(); fn custody_home_numeric_finish();
 fn custody_home_bitwise_raw(); fn custody_home_bitwise_observed();
 fn custody_method_receiver(builder:u64,receiver:u64)->u64;
 fn custody_prefix_reset(present:u64,fail:u64); fn custody_prefix_check(present:u64,fail:u64);
 fn molt_dec_ref_obj(value:u64);
}
fn main(){unsafe{
 for (mode,run) in [(0,custody_direct as unsafe extern "C" fn(u64,u64)->u64),(0,custody_internal),(1,custody_guarded),(1,custody_dynamic),(1,custody_method),(1,custody_loaded_method)]{
  for _ in 0..2{for fail in [0,1,2]{custody_reset(mode,fail,0);let word=run(0x501,0x502);assert_eq!(word,if fail==0{0x200}else{NONE});molt_dec_ref_obj(word);custody_check(fail,2,0);}}
 }
 for run in [custody_bind as unsafe extern "C" fn(u64)->u64,custody_indirect]{for fail in [0,1]{custody_reset(2,fail,0);let word=run(0x501);assert_eq!(word,if fail==0{0x200}else{NONE});molt_dec_ref_obj(word);custody_check(fail,1,0);}}
 custody_reset(0,0,1);assert_eq!(custody_direct(0x501,0x502),NONE);custody_check(0,2,1);
 for fail in 0..=4{custody_reset(3,fail,0);assert_eq!(custody_loop(),NONE);custody_loop_check(fail);}
 for fail in [0,1]{custody_home_reset(fail);custody_home_store();custody_home_finish(fail);}
 for fail in [0,1]{custody_home_reset(fail);custody_home_store_alias();custody_home_finish(fail);}
 custody_home_reset(0);custody_home_numeric();custody_home_numeric_finish();
 custody_home_reset(0);custody_home_bitwise_raw();custody_home_numeric_finish();
 for fail in [0,1]{custody_home_reset(fail);custody_home_bitwise_observed();custody_home_finish(fail);}
 for present in [0,1]{for fail in [0,1]{custody_prefix_reset(present,fail);assert_eq!(custody_method_receiver(0x777,if present==0{METHOD_NULL}else{0x501}),NONE);custody_prefix_check(present,fail);}}
}}
"#;
