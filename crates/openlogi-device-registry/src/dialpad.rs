//! Known Logitech dialpad identities.
//!
//! The MX Creative Dialpad, the MX Creative Console's companion to the
//! keypad, speaks ordinary HID++ and is handled by the generic device layer.
//! It is listed because its `0x0005` `DeviceType` names nothing OpenLogi
//! models, and over Logi Bolt the pairing register calls it a headset — so
//! neither protocol source gives it a kind, and kind decides the capture
//! plane. This table is the kind claim both paths fall back on, which
//! `openlogi-device` converts to a `DeviceKind` at its own boundary.
//!
//! Unlike [`crate::keypad`] there is no LCD panel to describe: the dial,
//! the roller and the four keys are all reported over HID++.

use crate::LOGITECH_VENDOR_ID;

/// A known Logitech dialpad identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialpadDescriptor {
    /// USB vendor ID.
    pub vendor_id: u16,
    /// USB / Bluetooth product ID. Over Logi Bolt the wireless product ID is
    /// the same value.
    pub product_id: u16,
    /// Marketed product name.
    pub name: &'static str,
    /// The `0x1b04` control each key reports, in face order: the two keys
    /// top left beside the roller, then the bottom-left and bottom-right keys
    /// beside the dial. A product fact like the keypad's panel geometry —
    /// the top pair happen to report the classic Back and Forward CIDs.
    pub keys: [u16; 4],
}

/// The MX Creative Dialpad.
pub const MX_CREATIVE_DIALPAD: DialpadDescriptor = DialpadDescriptor {
    vendor_id: LOGITECH_VENDOR_ID,
    product_id: 0xbc00,
    name: "MX Creative Dialpad",
    keys: [0x0053, 0x0056, 0x0059, 0x005a],
};

/// All dialpad identities OpenLogi recognizes.
pub const DIALPADS: &[DialpadDescriptor] = &[
    // MX Creative Dialpad — Bluetooth LE or Logi Bolt, HID++ 4.5. A large dial
    // and a roller on HID++ `0x4610`, four divertable keys on `0x1b04`.
    MX_CREATIVE_DIALPAD,
];

/// Finds a dialpad descriptor by its USB / Bluetooth identity.
#[must_use]
pub fn find_dialpad(vendor_id: u16, product_id: u16) -> Option<&'static DialpadDescriptor> {
    DIALPADS
        .iter()
        .find(|device| device.vendor_id == vendor_id && device.product_id == product_id)
}

/// Finds a dialpad descriptor by the wireless product ID a Logi Bolt receiver
/// reports for a paired device.
#[must_use]
pub fn find_dialpad_by_wpid(wpid: u16) -> Option<&'static DialpadDescriptor> {
    find_dialpad(LOGITECH_VENDOR_ID, wpid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialpad_is_found_by_usb_identity_and_by_wpid() {
        let dialpad = find_dialpad(LOGITECH_VENDOR_ID, 0xbc00).expect("MX Creative Dialpad");
        assert_eq!(dialpad.name, "MX Creative Dialpad");
        assert_eq!(find_dialpad_by_wpid(0xbc00), Some(dialpad));
        assert_eq!(find_dialpad(0x1234, 0xbc00), None, "another vendor");
        assert_eq!(
            find_dialpad_by_wpid(0xc354),
            None,
            "the keypad is not a dialpad"
        );
    }
}
