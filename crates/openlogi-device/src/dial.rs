//! HID++ feature `0x4610` — the MX Creative Dialpad's large dial and roller.
//!
//! Logitech publishes no spec for this feature. What is used here was observed
//! on an MX Creative Dialpad (Bluetooth LE, firmware RBO07.00_B0016) and
//! matches the MIT-licensed `node-logitech-mx-creative-console` driver:
//!
//! - Neither control reports anything until its reporting is switched on:
//!   function 3 with `[dial, 0x01]`, one call per control.
//! - Each rotation then arrives as an unsolicited function-0 report whose
//!   first parameter is the control (0 = roller, 1 = large dial) and whose
//!   second is a signed step count since the previous report. The parameters
//!   after it change on every report and look like a timestamp.
//! - Clockwise on the large dial and upward (away from the user) on the
//!   roller are positive.
//!
//! The large dial reports about fifteen steps per perceived click, the roller
//! about eight. Switching reporting back off is assumed to be `[dial, 0x00]`:
//! the device accepts it, but nothing confirms it is the inverse.

use std::sync::Arc;

use hidpp::{
    channel::HidppChannel,
    nibble::U4,
    protocol::v20::{self, Hidpp20Error},
};

use crate::thumbwheel::WheelResolution;

/// The dial feature's HID++ ID.
pub const FEATURE_ID: u16 = 0x4610;

/// Function that switches one control's rotation reporting on or off.
const FN_SET_REPORTING: u8 = 3;
/// `FN_SET_REPORTING` byte 1: report this control's rotation.
const REPORTING_ON: u8 = 0x01;
/// `FN_SET_REPORTING` byte 1: stop reporting (assumed; see the module docs).
const REPORTING_OFF: u8 = 0x00;

/// One of the Dialpad's two rotary controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DialControl {
    /// The small roller at the top right; positive is upward, away from the
    /// user.
    Roller,
    /// The large dial in the middle; positive is clockwise.
    Dial,
}

impl DialControl {
    /// Both controls, in wire order.
    pub const ALL: [Self; 2] = [Self::Roller, Self::Dial];

    /// The control's number on the wire.
    #[must_use]
    pub const fn wire_index(self) -> u8 {
        match self {
            Self::Roller => 0,
            Self::Dial => 1,
        }
    }

    fn from_wire(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Roller),
            1 => Some(Self::Dial),
            _ => None,
        }
    }

    /// What one perceived click measures, as a wheel resolution: one native
    /// scroll tick per click. Measured, not reported — the feature has no
    /// known info function.
    #[must_use]
    pub const fn resolution(self) -> WheelResolution {
        let steps_per_click = match self {
            Self::Roller => 8,
            Self::Dial => 15,
        };
        WheelResolution {
            native_res: 1,
            diverted_res: steps_per_click,
        }
    }
}

/// A decoded rotation report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialEvent {
    /// Which control turned.
    pub control: DialControl,
    /// Steps since the control's previous report; positive is clockwise on
    /// the dial and upward on the roller.
    pub delta: i8,
}

/// Decode a channel message into a [`DialEvent`] when it is the unsolicited
/// rotation report (function `0`) for `(device_index, feature_index)`.
///
/// Returns `None` for request responses (`software_id != 0`), messages from
/// another device or feature, and a control number this module does not know.
#[must_use]
pub fn decode_event(msg: &v20::Message, device_index: u8, feature_index: u8) -> Option<DialEvent> {
    let header = msg.header();
    if header.device_index != device_index
        || header.feature_index != feature_index
        || header.software_id.to_lo() != 0
        || header.function_id.to_lo() != 0
    {
        return None;
    }
    let p = msg.extend_payload();
    Some(DialEvent {
        control: DialControl::from_wire(p[0])?,
        delta: i8::from_be_bytes([p[1]]),
    })
}

/// Dial feature accessor bound to one device and resolved feature index.
///
/// Construct with the feature index from the device's root feature
/// (`get_feature(`[`FEATURE_ID`]`)`).
#[derive(Clone)]
pub struct Dials {
    chan: Arc<HidppChannel>,
    device_index: u8,
    feature_index: u8,
}

impl Dials {
    /// Bind the feature to `(device_index, feature_index)` on `chan`.
    #[must_use]
    pub fn new(chan: Arc<HidppChannel>, device_index: u8, feature_index: u8) -> Self {
        Self {
            chan,
            device_index,
            feature_index,
        }
    }

    /// The feature index this accessor talks to — used to match unsolicited
    /// events in [`decode_event`].
    #[must_use]
    pub fn feature_index(&self) -> u8 {
        self.feature_index
    }

    /// Switch on rotation reporting for every control.
    pub async fn enable(&self) -> Result<(), Hidpp20Error> {
        for control in DialControl::ALL {
            self.set_reporting(control, REPORTING_ON).await?;
        }
        Ok(())
    }

    /// Switch rotation reporting back off for every control, attempting each
    /// even when an earlier one fails; the first failure is returned.
    pub async fn disable(&self) -> Result<(), Hidpp20Error> {
        let mut result = Ok(());
        for control in DialControl::ALL {
            let written = self.set_reporting(control, REPORTING_OFF).await;
            if result.is_ok() {
                result = written;
            }
        }
        result
    }

    async fn set_reporting(&self, control: DialControl, value: u8) -> Result<(), Hidpp20Error> {
        let mut params = [0u8; 16];
        params[0] = control.wire_index();
        params[1] = value;
        self.chan
            .send_v20(v20::Message::Long(
                v20::MessageHeader {
                    device_index: self.device_index,
                    feature_index: self.feature_index,
                    function_id: U4::from_lo(FN_SET_REPORTING),
                    software_id: self.chan.get_sw_id(),
                },
                params,
            ))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long report exactly as hidraw delivered it, minus the report ID:
    /// device index, feature index, function/software nibble, payload.
    fn report(bytes: &[u8]) -> v20::Message {
        let mut payload = [0u8; 16];
        payload[..bytes.len() - 3].copy_from_slice(&bytes[3..]);
        v20::Message::Long(
            v20::MessageHeader {
                device_index: bytes[0],
                feature_index: bytes[1],
                function_id: U4::from_lo(bytes[2] >> 4),
                software_id: U4::from_lo(bytes[2] & 0x0f),
            },
            payload,
        )
    }

    #[test]
    fn decodes_captured_rotation_reports() {
        // `11 ff 0d 00 01 01 00 08 4d a1`: the large dial, one step clockwise.
        let dial = report(&[0xff, 0x0d, 0x00, 0x01, 0x01, 0x00, 0x08, 0x4d, 0xa1]);
        assert_eq!(
            decode_event(&dial, 0xff, 0x0d),
            Some(DialEvent {
                control: DialControl::Dial,
                delta: 1,
            })
        );
        // The roller, three steps down (toward the user).
        let roller = report(&[0xff, 0x0d, 0x00, 0x00, 0xfd, 0x00, 0x09, 0x10, 0x22]);
        assert_eq!(
            decode_event(&roller, 0xff, 0x0d),
            Some(DialEvent {
                control: DialControl::Roller,
                delta: -3,
            })
        );
    }

    #[test]
    fn ignores_responses_foreign_messages_and_unknown_controls() {
        // The enable call's answer carries its software ID.
        let answer = report(&[0xff, 0x0d, 0x3b, 0x01, 0x01]);
        assert_eq!(decode_event(&answer, 0xff, 0x0d), None);
        let other_feature = report(&[0xff, 0x0a, 0x00, 0x01, 0x01]);
        assert_eq!(decode_event(&other_feature, 0xff, 0x0d), None);
        let other_device = report(&[0x01, 0x0d, 0x00, 0x01, 0x01]);
        assert_eq!(decode_event(&other_device, 0xff, 0x0d), None);
        let unknown_control = report(&[0xff, 0x0d, 0x00, 0x02, 0x01]);
        assert_eq!(decode_event(&unknown_control, 0xff, 0x0d), None);
    }

    #[test]
    fn a_click_is_one_native_tick() {
        for control in DialControl::ALL {
            let resolution = control.resolution();
            let per_click = f64::from(resolution.diverted_res) * resolution.native_per_increment();
            assert!((per_click - 1.0).abs() < 1e-9, "{control:?}");
        }
        assert_eq!(DialControl::Roller.wire_index(), 0);
        assert_eq!(DialControl::Dial.wire_index(), 1);
    }
}
