//! A device's HID report descriptor, read the way Windows' HID parser reads it: one entry per top-level collection,
//! with its usage, the byte lengths of its longest input, output and feature report as HIDP_CAPS gives them (the
//! report ID byte counted, also when the device has no report IDs), and its fields. macOS hands an interface over as
//! one device with one descriptor; the collections, as Windows makes them of it, are worked out here.

/// One top-level collection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collection {
    pub usage_page: u16,
    pub usage: u16,
    pub input_length: usize,
    pub output_length: usize,
    pub feature_length: usize,
    /// The report ID of the first output field that carries data (value fields before buttons), as the Windows layer
    /// takes it from the value and button caps.
    pub output_report_id: u8,
    /// The IDs of its input reports (empty when the descriptor has no report IDs).
    pub input_ids: Vec<u8>,
    pub fields: Vec<Field>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Input,
    Output,
    Feature,
}

/// One main item: `count` values of `size` bits from bit `offset` of the report's data (the report ID byte not
/// counted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub kind: Kind,
    pub report_id: u8,
    pub offset: usize,
    pub size: usize,
    pub count: usize,
    /// Usage page in the high 16 bits, usage in the low ones; the last one stands for the values beyond the list.
    pub usages: Vec<u32>,
    pub logical_min: i32,
    pub logical_max: i32,
    pub constant: bool,
    pub variable: bool,
}

impl Field {
    /// Where the value with this usage starts (bits from the start of the report's data), if the field has it.
    pub fn bit_of(&self, usage_page: u16, usage: u16) -> Option<usize> {
        let wanted = ((usage_page as u32) << 16) | usage as u32;
        let index = self.usages.iter().position(|&u| u == wanted)?;
        (index < self.count).then(|| self.offset + index * self.size)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Descriptor {
    pub collections: Vec<Collection>,
    /// Report IDs are in use: every report starts with its ID byte.
    pub uses_report_ids: bool,
}

#[derive(Clone, Copy, Default)]
struct Globals {
    usage_page: u16,
    logical_min: i32,
    logical_max: i32,
    report_size: usize,
    report_id: u8,
    report_count: usize,
}

/// Bits used so far by each report: (kind, report ID) and its length in bits.
type Lengths = Vec<((Kind, u8), usize)>;

/// More usages than a range can sensibly hold for one field are not expanded.
const MAX_USAGES: u32 = 1024;

pub fn parse(bytes: &[u8]) -> Descriptor {
    let mut result = Descriptor::default();
    let mut globals = Globals::default();
    let mut stack: Vec<Globals> = Vec::new();
    let mut usages: Vec<u32> = Vec::new();
    let (mut usage_min, mut usage_max): (Option<u32>, Option<u32>) = (None, None);
    let mut depth = 0usize;
    let mut lengths: Lengths = Vec::new();
    // The top-level collection each report belongs to
    let mut owner: Vec<((Kind, u8), usize)> = Vec::new();

    let mut i = 0;
    while i < bytes.len() {
        let prefix = bytes[i];
        if prefix == 0xFE {
            // A long item: its size, its tag, its data
            let size = bytes.get(i + 1).copied().unwrap_or(0) as usize;
            i += 3 + size;
            continue;
        }
        let size = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let Some(data) = bytes.get(i + 1..i + 1 + size) else { break };
        i += 1 + size;
        let unsigned = data.iter().rev().fold(0u32, |v, &b| (v << 8) | b as u32);
        let signed = match size {
            1 => data[0] as i8 as i32,
            2 => i16::from_le_bytes([data[0], data[1]]) as i32,
            4 => unsigned as i32,
            _ => 0,
        };
        // A usage given in 4 bytes names its page itself
        let full_usage =
            |value: u32, page: u16| if size == 4 { value } else { ((page as u32) << 16) | (value & 0xFFFF) };
        let (kind, tag) = ((prefix >> 2) & 0x03, prefix >> 4);
        match kind {
            // Main
            0 => {
                match tag {
                    0x8 | 0x9 | 0xB => {
                        let field_kind = match tag {
                            0x8 => Kind::Input,
                            0x9 => Kind::Output,
                            _ => Kind::Feature,
                        };
                        let key = (field_kind, globals.report_id);
                        let bits = globals.report_size * globals.report_count;
                        let offset = match lengths.iter_mut().find(|l| l.0 == key) {
                            Some(l) => {
                                l.1 += bits;
                                l.1 - bits
                            }
                            None => {
                                lengths.push((key, bits));
                                0
                            }
                        };
                        let mut field_usages = usages.clone();
                        if let (Some(lo), Some(hi)) = (usage_min, usage_max) {
                            if hi >= lo && hi - lo < MAX_USAGES {
                                field_usages.extend(lo..=hi);
                            }
                        }
                        if result.collections.is_empty() {
                            // A field outside any collection: Windows puts it in a collection of its own
                            result.collections.push(Collection::default());
                        }
                        let top = result.collections.len() - 1;
                        if !owner.iter().any(|o| o.0 == key) {
                            owner.push((key, top));
                        }
                        let owner_of = owner.iter().find(|o| o.0 == key).map_or(top, |o| o.1);
                        let field = Field {
                            kind: field_kind,
                            report_id: globals.report_id,
                            offset,
                            size: globals.report_size,
                            count: globals.report_count,
                            usages: field_usages,
                            logical_min: globals.logical_min,
                            logical_max: globals.logical_max,
                            constant: unsigned & 0x01 != 0,
                            variable: unsigned & 0x02 != 0,
                        };
                        result.collections[owner_of].fields.push(field);
                    }
                    0xA => {
                        if depth == 0 {
                            let usage = usages.first().copied().or(usage_min).unwrap_or(0);
                            result.collections.push(Collection {
                                usage_page: (usage >> 16) as u16,
                                usage: usage as u16,
                                ..Default::default()
                            });
                        }
                        depth += 1;
                    }
                    0xC => depth = depth.saturating_sub(1),
                    _ => {}
                }
                usages.clear();
                (usage_min, usage_max) = (None, None);
            }
            // Global
            1 => match tag {
                0x0 => globals.usage_page = unsigned as u16,
                0x1 => globals.logical_min = signed,
                0x2 => globals.logical_max = signed,
                0x7 => globals.report_size = unsigned as usize,
                0x8 => {
                    globals.report_id = unsigned as u8;
                    result.uses_report_ids = true;
                }
                0x9 => globals.report_count = unsigned as usize,
                0xA => stack.push(globals),
                0xB => globals = stack.pop().unwrap_or(globals),
                _ => {}
            },
            // Local
            2 => match tag {
                0x0 => usages.push(full_usage(unsigned, globals.usage_page)),
                0x1 => usage_min = Some(full_usage(unsigned, globals.usage_page)),
                0x2 => usage_max = Some(full_usage(unsigned, globals.usage_page)),
                _ => {}
            },
            _ => {}
        }
    }

    for (index, collection) in result.collections.iter_mut().enumerate() {
        let longest = |kind: Kind| {
            owner
                .iter()
                .filter(|o| o.1 == index && o.0 .0 == kind)
                .filter_map(|o| lengths.iter().find(|l| l.0 == o.0).map(|l| l.1))
                .max()
                .map_or(0, |bits| bits.div_ceil(8) + 1)
        };
        collection.input_length = longest(Kind::Input);
        collection.output_length = longest(Kind::Output);
        collection.feature_length = longest(Kind::Feature);
        let data = |value: bool| {
            collection
                .fields
                .iter()
                .find(|f| f.kind == Kind::Output && !f.constant && f.count > 0 && (f.variable && f.size > 1) == value)
        };
        collection.output_report_id = data(true).or_else(|| data(false)).map_or(0, |f| f.report_id);
        if result.uses_report_ids {
            let mut ids: Vec<u8> =
                owner.iter().filter(|o| o.1 == index && o.0 .0 == Kind::Input).map(|o| o.0 .1).collect();
            ids.sort_unstable();
            ids.dedup();
            collection.input_ids = ids;
        }
    }
    result
}

/// A field's value from a report (its first byte the report ID): `size` bits from bit `bit` of the data, unsigned,
/// as HidP_GetUsageValue gives it.
pub fn value_at(report: &[u8], bit: usize, size: usize) -> Option<u32> {
    if size == 0 || size > 32 {
        return None;
    }
    let data = report.get(1..)?;
    if (bit + size).div_ceil(8) > data.len() {
        return None;
    }
    let mut value = 0u64;
    for n in 0..size {
        let at = bit + n;
        if data[at / 8] >> (at % 8) & 1 != 0 {
            value |= 1 << n;
        }
    }
    Some(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Logitech Unifying receiver's third interface: the HID++ short (7 bytes) and long (20 bytes) reports, each in
    /// a vendor collection of its own, and the DJ reports.
    const UNIFYING_MI2: &[u8] = &[
        0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x10, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x26, 0xFF, 0x00,
        0x09, 0x01, 0x81, 0x00, 0x09, 0x01, 0x91, 0x00, 0xC0, 0x06, 0x00, 0xFF, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x11,
        0x95, 0x13, 0x75, 0x08, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x09, 0x02, 0x81, 0x00, 0x09, 0x02, 0x91, 0x00, 0xC0,
        0x06, 0x00, 0xFF, 0x09, 0x04, 0xA1, 0x01, 0x85, 0x20, 0x95, 0x0E, 0x75, 0x08, 0x15, 0x00, 0x26, 0xFF, 0x00,
        0x09, 0x41, 0x81, 0x00, 0x09, 0x41, 0x91, 0x00, 0x85, 0x21, 0x95, 0x1F, 0x09, 0x42, 0x81, 0x00, 0x09, 0x42,
        0x91, 0x00, 0xC0,
    ];

    #[test]
    fn collections_and_lengths_as_windows_has_them() {
        let d = parse(UNIFYING_MI2);
        assert!(d.uses_report_ids);
        let shape: Vec<_> = d
            .collections
            .iter()
            .map(|c| (c.usage_page, c.usage, c.input_length, c.output_length, c.feature_length, c.output_report_id))
            .collect();
        // What Windows reports for these three collections (HidP_GetCaps)
        assert_eq!(shape, [(0xFF00, 1, 7, 7, 0, 0x10), (0xFF00, 2, 20, 20, 0, 0x11), (0xFF00, 4, 32, 32, 0, 0x20)]);
        assert_eq!(d.collections[2].input_ids, [0x20, 0x21]);
    }

    /// A gamepad without report IDs: the lengths still count the (zero) report ID byte.
    const NO_IDS: &[u8] = &[
        0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x05, 0x09, 0x19, 0x01, 0x29, 0x0C, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
        0x95, 0x0C, 0x81, 0x02, 0x95, 0x04, 0x81, 0x03, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x26, 0xFF,
        0x00, 0x75, 0x08, 0x95, 0x02, 0x81, 0x02, 0x06, 0x00, 0xFF, 0x09, 0x20, 0x95, 0x05, 0x91, 0x02, 0xC0,
    ];

    #[test]
    fn a_descriptor_without_report_ids() {
        let d = parse(NO_IDS);
        assert!(!d.uses_report_ids);
        let c = &d.collections[0];
        assert_eq!((c.usage_page, c.usage, c.input_length, c.output_length, c.feature_length), (1, 5, 5, 6, 0));
        assert!(c.input_ids.is_empty());
        // The X axis: after 16 bits of buttons and padding
        let x = c.fields.iter().find_map(|f| f.bit_of(0x01, 0x30)).unwrap();
        let y = c.fields.iter().find_map(|f| f.bit_of(0x01, 0x31)).unwrap();
        assert_eq!((x, y), (16, 24));
        assert_eq!(c.fields[0].usages.len(), 12, "the button range");
    }

    /// The standard battery field of a 2.4 GHz receiver: Generic Device Controls, Battery Strength, in feature report 7
    /// after a byte of something else.
    const BATTERY: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x01, 0x09, 0x30, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0xC0, 0x05,
        0x06, 0x09, 0x00, 0xA1, 0x01, 0x85, 0x07, 0x75, 0x08, 0x95, 0x01, 0x09, 0x21, 0xB1, 0x02, 0x09, 0x20, 0x15,
        0x00, 0x25, 0x64, 0xB1, 0x02, 0xC0,
    ];

    #[test]
    fn the_battery_field_and_its_value() {
        let d = parse(BATTERY);
        assert_eq!(d.collections.len(), 2);
        let c = &d.collections[1];
        assert_eq!((c.usage_page, c.feature_length), (0x06, 3));
        let field = c.fields.iter().find(|f| f.kind == Kind::Feature && f.bit_of(0x06, 0x20).is_some()).unwrap();
        assert_eq!(
            (field.report_id, field.bit_of(0x06, 0x20), field.logical_min, field.logical_max),
            (7, Some(8), 0, 100)
        );
        assert_eq!(value_at(&[7, 0x11, 85], 8, 8), Some(85));
        assert_eq!(value_at(&[7, 0x11], 8, 8), None, "a report too short has no value");
        assert_eq!(value_at(&[1, 0b1010_0000, 0b11], 5, 5), Some(0b11101));
    }

    #[test]
    fn a_one_byte_maximum_of_255_is_minus_one_as_on_windows() {
        let d = parse(&[
            0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x15, 0x00, 0x25, 0xFF, 0x75, 0x08, 0x95, 0x01, 0x09, 0x01, 0xB1,
            0x02, 0xC0,
        ]);
        let f = &d.collections[0].fields[0];
        assert_eq!((f.logical_min, f.logical_max), (0, -1));
    }

    #[test]
    fn push_pop_long_items_and_garbage_never_panic() {
        let d =
            parse(&[0x05, 0x0C, 0xA4, 0x05, 0x01, 0xB4, 0x09, 0x01, 0xA1, 0x01, 0xFE, 0x02, 0x10, 0xAA, 0xBB, 0xC0]);
        assert_eq!((d.collections[0].usage_page, d.collections[0].usage), (0x0C, 0x01));
        for len in 0..UNIFYING_MI2.len() {
            parse(&UNIFYING_MI2[..len]);
        }
        parse(&[0x27, 0xFF]);
        parse(&[0xC0, 0xC0, 0x81, 0x02]);
    }
}
