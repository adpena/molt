//! Logical frame shapes and reservation sizes.
//! A target computes physical arena occupancy only when it lowers an entry.

use crate::tir::ops::{AttrDict, AttrValue};

/// Bits 0..32 of a `trace_enter_slot` value: the frame's home count.
pub const FRAME_ENTRY_HOMES_MASK: i64 = 0xffff_ffff;
/// Set in a `trace_enter_slot` value when the frame clears its slots last to
/// first (CPython 3.14 and later; molt-runtime `FramePolicy::descending`).
pub const FRAME_ENTRY_DESCENDING: i64 = 1 << 32;
/// The `trace_enter_slot` value of a stateful activation's entry (a
/// generator, coroutine or async-generator poll): it links its task's
/// persistent record, whose bindings live in the task payload.
pub const FRAME_ENTRY_STATEFUL: i64 = 1 << 33;

/// What a synchronous frame entry declares: the `trace_enter_slot` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameShape {
    pub homes: u32,
    pub descending: bool,
}

impl FrameShape {
    /// Decode a `trace_enter_slot` value.
    pub const fn from_entry_value(value: i64) -> Self {
        Self {
            homes: (value & FRAME_ENTRY_HOMES_MASK) as u32,
            descending: value & FRAME_ENTRY_DESCENDING != 0,
        }
    }

    pub const fn entry_value(self) -> i64 {
        self.homes as i64
            | if self.descending {
                FRAME_ENTRY_DESCENDING
            } else {
                0
            }
    }
}

/// How a compiled frame entry links its record: the decoded
/// `trace_enter_slot` value. An entry with no value is runtime-linked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameEntryKind {
    /// A synchronous function pushes and pops an arena record inline.
    Synchronous(FrameShape),
    /// A stateful activation links and unlinks its task's persistent record.
    Stateful,
}

impl FrameEntryKind {
    pub const fn from_entry_value(value: i64) -> Self {
        if value & FRAME_ENTRY_STATEFUL != 0 {
            Self::Stateful
        } else {
            Self::Synchronous(FrameShape::from_entry_value(value))
        }
    }

    /// The synchronous shape, or `None` for a stateful entry.
    pub const fn shape(self) -> Option<FrameShape> {
        match self {
            Self::Synchronous(shape) => Some(shape),
            Self::Stateful => None,
        }
    }
}

/// The virtual frames and homes concurrently live at a point in a region.
/// The executing thread already accounts for the physical ancestors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FramePrefix {
    pub homes: u32,
    pub depth: u32,
}

impl FramePrefix {
    const VIRTUAL_LEVEL_ATTR: &str = "frame_virtual_lexical_level";
    const HOMES_ATTR: &str = "frame_reservation_homes";
    const DEPTH_ATTR: &str = "frame_reservation_depth";

    pub fn enter(self, shape: FrameShape) -> Option<Self> {
        Some(Self {
            homes: self.homes.checked_add(shape.homes)?,
            depth: self.depth.checked_add(1)?,
        })
    }

    pub fn reservation_attrs(self) -> [(&'static str, AttrValue); 2] {
        [
            (Self::HOMES_ATTR, AttrValue::Int(i64::from(self.homes))),
            (Self::DEPTH_ATTR, AttrValue::Int(i64::from(self.depth))),
        ]
    }

    /// A framed fast path can omit an unobserved lexical exception level
    /// only when its physical admission would leave the active stack aligned.
    pub fn require_virtual_level(attrs: &mut AttrDict) {
        attrs.insert(Self::VIRTUAL_LEVEL_ATTR.into(), AttrValue::Bool(true));
    }

    pub fn requires_virtual_level(attrs: &AttrDict) -> bool {
        matches!(
            attrs.get(Self::VIRTUAL_LEVEL_ATTR),
            Some(AttrValue::Bool(true))
        )
    }

    /// Consume the requirements that a quiescent region will check at its entry.
    /// Keep the original probe untouched on every path outside that region.
    pub fn take_thread_requirements(attrs: &mut AttrDict) -> Option<(Self, bool)> {
        let prefix = Self::reservation_from_attrs(attrs)?;
        let virtual_level = Self::requires_virtual_level(attrs);
        attrs.remove(Self::HOMES_ATTR);
        attrs.remove(Self::DEPTH_ATTR);
        attrs.remove(Self::VIRTUAL_LEVEL_ATTR);
        Some((prefix, virtual_level))
    }

    const REGION_CALLABLE_ATTR: &str = "frame_region_callable_admitted";

    /// A region-entry probe checked this invariant callable and code slot.
    /// Per-invocation observations of thread state still run at the site.
    pub fn admit_region_callable(attrs: &mut AttrDict) {
        attrs.insert(Self::REGION_CALLABLE_ATTR.into(), AttrValue::Bool(true));
    }

    pub fn region_callable_admitted(attrs: &AttrDict) -> bool {
        matches!(
            attrs.get(Self::REGION_CALLABLE_ATTR),
            Some(AttrValue::Bool(true))
        )
    }

    pub fn reservation_from_attrs(attrs: &AttrDict) -> Option<Self> {
        // An older per-entry ancestor count is not a region reservation.
        if [
            "frame_prefix_bytes",
            "frame_prefix_homes",
            "frame_prefix_depth",
        ]
        .iter()
        .any(|&field| attrs.contains_key(field))
        {
            return None;
        }
        match (attrs.get(Self::HOMES_ATTR), attrs.get(Self::DEPTH_ATTR)) {
            (None, None) => Some(Self::default()),
            (Some(AttrValue::Int(homes)), Some(AttrValue::Int(depth))) => {
                let homes = u32::try_from(*homes).ok()?;
                let depth = u32::try_from(*depth).ok()?;
                (depth != 0 || homes == 0).then_some(Self { homes, depth })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_entry_value_names_a_stateful_entry_apart_from_every_shape() {
        assert_eq!(
            FrameEntryKind::from_entry_value(FRAME_ENTRY_STATEFUL),
            FrameEntryKind::Stateful
        );
        for shape in [
            FrameShape {
                homes: 0,
                descending: false,
            },
            FrameShape {
                homes: u32::MAX,
                descending: true,
            },
        ] {
            assert_eq!(
                FrameEntryKind::from_entry_value(shape.entry_value()),
                FrameEntryKind::Synchronous(shape)
            );
        }
    }

    #[test]
    fn an_entry_value_round_trips_the_frame_shape() {
        for shape in [
            FrameShape {
                homes: 0,
                descending: false,
            },
            FrameShape {
                homes: 5,
                descending: true,
            },
            FrameShape {
                homes: u32::MAX,
                descending: true,
            },
        ] {
            assert!(shape.entry_value() >= 0);
            assert_eq!(FrameShape::from_entry_value(shape.entry_value()), shape);
        }
    }

    #[test]
    fn a_prefix_counts_homes_and_frames_before_the_next_entry() {
        let prefix = FramePrefix::default()
            .enter(FrameShape {
                homes: 3,
                descending: true,
            })
            .unwrap()
            .enter(FrameShape {
                homes: 0,
                descending: false,
            })
            .unwrap();
        assert_eq!(prefix, FramePrefix { homes: 3, depth: 2 });
        let attrs: AttrDict = prefix
            .reservation_attrs()
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect();
        assert_eq!(FramePrefix::reservation_from_attrs(&attrs), Some(prefix));
        assert_eq!(
            FramePrefix::reservation_from_attrs(&AttrDict::default()),
            Some(FramePrefix::default())
        );
        let mut incomplete = attrs.clone();
        incomplete.remove(FramePrefix::HOMES_ATTR);
        assert!(FramePrefix::reservation_from_attrs(&incomplete).is_none());
        incomplete.insert("frame_prefix_bytes".into(), AttrValue::Int(96));
        assert!(FramePrefix::reservation_from_attrs(&incomplete).is_none());
        assert!(
            prefix
                .enter(FrameShape {
                    homes: u32::MAX,
                    descending: false
                })
                .is_none()
        );
    }
}
