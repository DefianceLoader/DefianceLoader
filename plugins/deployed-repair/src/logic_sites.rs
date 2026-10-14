//! RTTI-anchored logic.dll sites for self-repair and its synchronous stop path.

use defiance_core::pattern;
use defiance_core::sites::{sig, Image, Signature};

const SELF_REPAIR_ORDER: &str = ".?AVAiSelfRepairOrder@Leonardo@@";
const TRANSPORT_HELPER: &str = ".?AVTransportHelper@Leonardo@@";

/// The whole instructions displaced by the self-repair entry hook.
pub(crate) const ORDER_BEFORE: &[u8] =
    b"\x48\x89\x4c\x24\x08\x53\x55\x56\x57\x41\x54\x41\x55\x41\x56\x41\x57";

const ORDER_CREW_HELPER_CALL: usize = 0x446;
const ORDER_REPAIR_HELPER_CALL: usize = 0x4e4;
const CREW_HELPER_CALL_BYTES: &[u8] = b"\xff\x90\x00\x02\x00\x00";
const REPAIR_HELPER_CALL_BYTES: &[u8] = b"\xff\x90\xa0\x02\x00\x00";
const STOP_CALL_OFFSET: usize = 0x240;

// The complete entity/AI lookup and four deployment-state predicates bind
// this call to the stop path inside TransportHelper slot 89. Its continuation
// reads the entity again without consuming a return value from the factory.
const STOP_DEPLOYMENT_CALL: Signature = sig(
    "TransportHelper deployment cleanup",
    &[(
        concat!(
            "488b4f20488b01ff90b0000000488b58284885db7451",
            "488b03ba18000000488bcbff506884c07536",
            "488b03ba1a000000488bcbff506884c07524",
            "488b03ba19000000488bcbff506884c07512",
            "488b03ba1c000000488bcbff506884c07409",
            "488b4f20e8????????",
            "488b4f20488b01ff90b0000000488b48284885c9740e",
        ),
        0x62,
    )],
);

// This wrapper constructs Leonardo::AiStopRechargeOrder. Its virtual facet
// lookup distinguishes it from the order-list getter elsewhere in slot 89.
const STOP_RECHARGE_FACTORY: Signature = sig(
    "AiStopRechargeOrder factory",
    &[(
        "4056574883ec38488b01488bf9ff90b0000000488b70284885f60f84????????488b07488bcf48895c246048896c2468",
        0,
    )],
);

const RECHARGE_STATE: &str = ".?AVAiRechargeState@Leonardo@@";
const SELF_REPAIR_STATE: &str = ".?AVAiSelfRepairState@Leonardo@@";
const IDLE_STATE: &str = ".?AVAiIdleState@Leonardo@@";
const IDLE_HANDLER_SPAN: usize = 0x600;

/// The whole instructions displaced by the recharge-state order hook.
pub(crate) const RECHARGE_BEFORE: &[u8] =
    b"\x48\x89\x5c\x24\x08\x48\x89\x74\x24\x20\x48\x89\x54\x24\x10";

// (order handler slot, state id slot): the 2025-12-23 builds lack one
// earlier virtual, so both sit one slot lower there.
const STATE_LAYOUTS: [(usize, usize); 2] = [(16, 9), (15, 8)];
const RECHARGE_ID_CALL: usize = 0x32;

// AiRechargeState's order handler reads the task's state id, forwards only
// ids 9, 0x1d and 0x21 to its nested machine, and drops every other task
// unless it is ready to leave deployment.
const RECHARGE_HANDLER: Signature = sig(
    "AiRechargeState order handler",
    &[(
        concat!(
            "48895c240848897424204889542410574883ec20",
            "488bda488bf9488b0a4885c97409488b49104885c9750233c9",
            "488b01ff50??83f8210f87????????",
            "48b90002002002000000480fa3c1",
        ),
        0,
    )],
);

const SIT_ON_LAND_STATE: &str = ".?AVAiSitOnLandState@Leonardo@@";

/// The whole instructions displaced by the landed-state order hook.
pub(crate) const SIT_ON_LAND_BEFORE: &[u8] =
    b"\x48\x89\x5c\x24\x18\x48\x89\x54\x24\x10\x57\x48\x83\xec\x20";

/// The landed state's field that is non-null while its nested machine runs;
/// the stock handler then hands every task to that machine.
pub(crate) const SIT_ON_LAND_NESTED: usize = 0x40;
const SIT_ON_LAND_ID_CALL: usize = 0x42;
const SIT_ON_LAND_PUSH_CALL: usize = 0x6f;

// AiSitOnLandState's order handler (a landed helicopter) forwards task ids
// 9, 0x14, 0x15 and 0x20 to its own helper, pushes id 0x21 through the
// generic push helper, and drops every other task, self-repair included.
const SIT_ON_LAND_HANDLER: Signature = sig(
    "AiSitOnLandState order handler",
    &[(
        concat!(
            "48895c24184889542410574883ec20488bda488bf9",
            "4883794000740e4883c130e8????????e9????????",
            "488b0a4885c97409488b49104885c9750233c9",
            "488b01ff50??",
            "83e809742e83e80b742983e801742483e80b741f83f801755f",
            "488bd3488d4c2430e8????????488bd0488bcfe8????????",
        ),
        0,
    )],
);

// The state id getter: `mov eax, imm32; ret`.
const STATE_ID: Signature = sig("AiSelfRepairState id", &[("b8????????c3", 0)]);

// The generic helper AiIdleState's order handler ends with: it hands a task
// state to the owning machine's push (vtable +0x68) and consumes the handle.
const PUSH_TASK: Signature = sig(
    "AI task push helper",
    &[(
        concat!(
            "48895c241848897424204889542410574883ec20488bda488b79104885ff74??",
            "488b7f104885ff74??488b0a4885c974??488379100074??488b07488b7068",
        ),
        0,
    )],
);

/// Validated hook entries and their original bytes, as logic.dll RVAs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sites {
    pub order: usize,
    pub order_before: Vec<u8>,
    pub stop: usize,
    pub stop_before: Vec<u8>,
    pub recharge: usize,
    pub sit_on_land: usize,
    pub push: usize,
    /// The task vtable offset of the state id getter.
    pub task_id: usize,
    pub self_repair_id: u32,
}

/// The recharge order handler, its vtable slot and the task id getter's slot.
fn recharge_handler(image: &Image<'_>) -> Result<(usize, usize, usize), String> {
    for (handler_slot, id_slot) in STATE_LAYOUTS {
        let Ok(handler) = image.method(RECHARGE_STATE, handler_slot) else {
            continue;
        };
        if matches_signature_at(image, handler, &RECHARGE_HANDLER)
            && image.image.get(handler + RECHARGE_ID_CALL) == Some(&((id_slot * 8) as u8))
        {
            return Ok((handler, handler_slot, id_slot));
        }
    }
    Err("AiRechargeState order handler does not match".into())
}

/// The landed-state order handler. It reads the task id through the same
/// slot as the recharge handler and pushes through the same helper.
fn sit_on_land_handler(
    image: &Image<'_>,
    handler_slot: usize,
    id_slot: usize,
    push: usize,
) -> Result<usize, String> {
    image.expect(
        "AiSitOnLandState id",
        image.method(SIT_ON_LAND_STATE, id_slot)?,
        b"\xb8\x13\x00\x00\x00\xc3",
    )?;
    let handler = image.method(SIT_ON_LAND_STATE, handler_slot)?;
    if !matches_signature_at(image, handler, &SIT_ON_LAND_HANDLER)
        || image.image.get(handler + SIT_ON_LAND_ID_CALL) != Some(&((id_slot * 8) as u8))
    {
        return Err("AiSitOnLandState order handler does not match".into());
    }
    if image.branch_target(handler + SIT_ON_LAND_PUSH_CALL) != Some(push) {
        return Err("AiSitOnLandState pushes through an unexpected helper".into());
    }
    Ok(handler)
}

/// The push helper, as the one matching call target in AiIdleState's handler.
fn push_helper(image: &Image<'_>, handler_slot: usize) -> Result<usize, String> {
    let idle = image.method(IDLE_STATE, handler_slot)?;
    let mut targets: Vec<usize> = (idle..idle + IDLE_HANDLER_SPAN)
        .filter(|&at| image.image.get(at) == Some(&0xe8))
        .filter_map(|at| image.branch_target(at))
        .filter(|&target| matches_signature_at(image, target, &PUSH_TASK))
        .collect();
    targets.sort_unstable();
    targets.dedup();
    match targets.as_slice() {
        [push] => Ok(*push),
        [] => Err("AiIdleState does not call the task push helper".into()),
        _ => Err("AiIdleState calls several task push helpers".into()),
    }
}

/// Resolve all native sites before the caller stages any hook.
pub(crate) fn resolve(image: &Image<'_>) -> Result<Sites, String> {
    let order = image.method(SELF_REPAIR_ORDER, 6)?;
    image.expect("self-repair order entry", order, ORDER_BEFORE)?;
    image.expect(
        "self-repair crew-exit helper call",
        order + ORDER_CREW_HELPER_CALL,
        CREW_HELPER_CALL_BYTES,
    )?;
    image.expect(
        "self-repair order helper call",
        order + ORDER_REPAIR_HELPER_CALL,
        REPAIR_HELPER_CALL_BYTES,
    )?;

    let helper_exit = image.method(TRANSPORT_HELPER, 89)?;
    let stop = helper_exit + STOP_CALL_OFFSET;
    if !matches_signature_at(image, stop, &STOP_DEPLOYMENT_CALL) {
        return Err("TransportHelper deployment cleanup does not match".into());
    }
    let target = image
        .branch_target(stop)
        .ok_or_else(|| "StopRecharge call target does not decode".to_owned())?;
    if !matches_signature_at(image, target, &STOP_RECHARGE_FACTORY) {
        return Err("TransportHelper calls an unexpected StopRecharge wrapper".into());
    }
    let stop_before = image
        .image
        .get(stop..stop + 5)
        .ok_or_else(|| "StopRecharge factory call runs off logic.dll".to_owned())?
        .to_vec();

    let (recharge, handler_slot, id_slot) = recharge_handler(image)?;
    let id_getter = image.method(SELF_REPAIR_STATE, id_slot)?;
    if !matches_signature_at(image, id_getter, &STATE_ID) {
        return Err("AiSelfRepairState id getter does not match".into());
    }
    let self_repair_id = image
        .u32(id_getter + 1)
        .ok_or_else(|| "AiSelfRepairState id runs off logic.dll".to_owned())?;
    let push = push_helper(image, handler_slot)?;
    let sit_on_land = sit_on_land_handler(image, handler_slot, id_slot, push)?;

    Ok(Sites {
        order,
        order_before: ORDER_BEFORE.to_vec(),
        stop,
        stop_before,
        recharge,
        sit_on_land,
        push,
        task_id: id_slot * 8,
        self_repair_id,
    })
}

fn matches_signature_at(image: &Image<'_>, site: usize, signature: &Signature) -> bool {
    signature.alternatives.iter().any(|(text, offset)| {
        let Some(start) = site.checked_sub(*offset) else {
            return false;
        };
        let Ok(pattern) = pattern::parse(text) else {
            return false;
        };
        let Some(end) = start.checked_add(pattern.len()) else {
            return false;
        };
        image.image.get(start..end).is_some_and(|bytes| {
            pattern
                .iter()
                .zip(bytes)
                .all(|(expected, actual)| expected.is_none_or(|byte| byte == *actual))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    const STOP_RVAS: [usize; 8] = [
        0x466c60, 0x466cf0, 0x4796b0, 0x479740, 0x479cf0, 0x479d80, 0x47d2a0, 0x47d330,
    ];

    const SIT_ON_LAND_RVAS: [usize; 8] = [
        0xf6730, 0xf67c0, 0xfd1a0, 0xfd230, 0xfd1a0, 0xfd230, 0xfd1a0, 0xfd230,
    ];

    #[test]
    fn resolves_the_stop_factory_on_all_reference_builds() {
        let mut checked = 0;
        for ((build, expected), sit_on_land) in BUILDS.iter().zip(STOP_RVAS).zip(SIT_ON_LAND_RVAS) {
            let Some(mapped) = reference(build, "logic.dll") else {
                continue;
            };
            let image = Image::mapped(&mapped);
            let sites = resolve(&image).unwrap_or_else(|error| panic!("{build}: {error}"));
            assert_eq!(sites.stop, expected);
            assert_eq!(sites.order_before.len(), ORDER_BEFORE.len());
            assert_eq!(sites.stop_before.len(), 5);
            assert!(image.branch_target(sites.stop).is_some_and(|target| {
                matches_signature_at(&image, target, &STOP_RECHARGE_FACTORY)
            }));
            assert_eq!(sites.self_repair_id, 0x2e, "{build}");
            assert!(
                image.starts_with(sites.recharge, RECHARGE_BEFORE),
                "{build}"
            );
            assert!(
                matches_signature_at(&image, sites.push, &PUSH_TASK),
                "{build}"
            );
            assert_eq!(sites.sit_on_land, sit_on_land, "{build}");
            assert!(
                image.starts_with(sites.sit_on_land, SIT_ON_LAND_BEFORE),
                "{build}"
            );
            checked += 1;
        }
        assert!(checked > 0, "no logic.dll reference builds are available");
    }

    #[test]
    fn refuses_changed_order_predicates_call_and_factory() {
        let Some(mapped) = reference(BUILDS[7], "logic.dll") else {
            return;
        };
        let base = Image::mapped(&mapped);
        let sites = resolve(&base).unwrap();
        let target = base.branch_target(sites.stop).unwrap();
        for at in [
            sites.order,
            sites.order + ORDER_CREW_HELPER_CALL,
            sites.order + ORDER_REPAIR_HELPER_CALL,
            sites.stop - 0x62,
            sites.stop - 0x22,
            sites.stop,
            sites.stop + 1,
            sites.stop + 5,
            target,
            sites.recharge,
            sites.recharge + 0x16,
            sites.recharge + RECHARGE_ID_CALL,
            sites.recharge + 0x3e,
            sites.push + 0x2f,
            sites.sit_on_land,
            sites.sit_on_land + 0x18,
            sites.sit_on_land + SIT_ON_LAND_ID_CALL,
            sites.sit_on_land + 0x45,
            sites.sit_on_land + SIT_ON_LAND_PUSH_CALL + 1,
        ] {
            let mut changed = base.image.to_vec();
            changed[at] ^= 1;
            assert!(
                resolve(&Image {
                    image: &changed,
                    base: base.base
                })
                .is_err(),
                "changed logic.dll+{at:#x}"
            );
        }
    }
}
