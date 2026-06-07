/// WASM Component Model detection and dispatch layer.
///
/// A WASM component can be detected by checking for the magic prefix `\\0asm`
/// followed by version 1 and type section markers. Raw modules are flat core
/// wasm with the `run_tool` export pattern used by the pre-component path.
/// Classification of a WASM binary: either a raw core module (flat ABI) or a
/// Component Model module (component section present).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    /// Raw core WASM module using the flat `run_tool(name_ptr,name_len,input_ptr,input_len) -> i32` ABI.
    Raw,
    /// WASM Component Model module with a `navi-plugin` component section.
    Component,
}

/// Detect whether a WASM binary is a Component Model module or a raw core
/// module by inspecting its magic prefix and component section.
///
/// Component Model modules begin with the standard WASM magic (`\0asm`) and
/// version 1 header, then contain a component-section (ID = 3 for
/// core modules, but components use section ID 0x0b = 3 for the
/// `component` section after the core module envelope). We detect the
/// component section by checking for the `0x0b` (component section) byte
/// at an appropriate position after the header.
///
/// If the binary is too short to contain a valid header, we return `Raw`
/// (the safest default).
pub fn detect_component_kind(wasm_bytes: &[u8]) -> ComponentKind {
    // Minimum for any valid WASM: 8-byte header.
    if wasm_bytes.len() < 8 {
        return ComponentKind::Raw;
    }

    // Check magic: 0x00 0x61 0x73 0x6d (`\0asm`)
    if wasm_bytes[0..4] != [0x00, 0x61, 0x73, 0x6d] {
        return ComponentKind::Raw;
    }

    // Version must be 1 (0x01 0x00 0x00 0x00).
    if wasm_bytes[4..8] != [0x01, 0x00, 0x00, 0x00] {
        return ComponentKind::Raw;
    }

    // Component Model: after the core WASM envelope, a component module has
    // a component section with ID = 0x0b. We scan for a section with ID 0x0b.
    //
    // However, the simpler heuristic used in practice is to look for the
    // `producers` custom section or the component start. Since both are rare
    // in raw modules, a more reliable approach is to check for the first
    // occurrence of the `0x0b` byte after the WASM header as a section ID.
    //
    // The WASM spec section IDs:
    //   1 = Type, 2 = Import, 3 = Function, 4 = Table, 5 = Memory,
    //   6 = Global, 7 = Export, 8 = Start, 9 = Element, 10 = Code,
    //   11 = Data, 12 = DataCount, 13 = Tag, 0x0b = Component (Component Model)
    //
    // For a component, the file begins with a core module envelope followed by
    // additional sections including the component section. Since raw modules
    // will never have a section with ID 0x0b (reserved for components), we
    // scan the section headers after the header.

    let mut pos = 8; // skip header
    while pos + 1 < wasm_bytes.len() {
        let section_id = wasm_bytes[pos];
        pos += 1;

        // Section length is a LEB128-encoded unsigned integer.
        let (size, end) = match decode_leb128(wasm_bytes, pos) {
            Some(v) => v,
            None => break, // malformed — treat as raw
        };
        pos = end;

        if section_id == 0x0b {
            return ComponentKind::Component;
        }

        // Skip past the section body.
        if pos + size as usize > wasm_bytes.len() {
            break;
        }
        pos += size as usize;
    }

    ComponentKind::Raw
}

/// Decode a WASM LEB128 unsigned integer starting at `pos`.
/// Returns `(value, new_pos)` or `None` if the input is too short.
fn decode_leb128(bytes: &[u8], pos: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut current = pos;
    loop {
        if current >= bytes.len() {
            return None;
        }
        let byte = bytes[current];
        result |= ((byte & 0x7f) as u64) << shift;
        current += 1;
        if byte & 0x80 == 0 {
            return Some((result, current));
        }
        shift += 7;
        if shift > 63 {
            return None; // overflow
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_wasm_header() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
        ]
    }

    #[test]
    fn detect_empty_is_raw() {
        assert_eq!(detect_component_kind(&[]), ComponentKind::Raw);
    }

    #[test]
    fn detect_too_short_is_raw() {
        assert_eq!(detect_component_kind(&[0x00, 0x61]), ComponentKind::Raw);
    }

    #[test]
    fn detect_header_only_is_raw() {
        assert_eq!(
            detect_component_kind(&raw_wasm_header()),
            ComponentKind::Raw
        );
    }

    #[test]
    fn detect_header_with_code_section_is_raw() {
        let mut bytes = raw_wasm_header();
        // Section 10 (Code), 2 bytes of body: 0x01 0x00 (empty function)
        bytes.extend_from_slice(&[10, 2, 1, 0]);
        assert_eq!(detect_component_kind(&bytes), ComponentKind::Raw);
    }

    #[test]
    fn detect_header_with_component_section() {
        let mut bytes = raw_wasm_header();
        // Section 0x0b (Component), 2 bytes of body: 0x00 0x00
        bytes.extend_from_slice(&[0x0b, 2, 0x00, 0x00]);
        assert_eq!(detect_component_kind(&bytes), ComponentKind::Component);
    }

    #[test]
    fn detect_malformed_leb128_treated_as_raw() {
        let mut bytes = raw_wasm_header();
        // Section 10 with a truncated LEB128 length
        bytes.extend_from_slice(&[10, 0xff, 0xff]);
        assert_eq!(detect_component_kind(&bytes), ComponentKind::Raw);
    }

    #[test]
    fn detect_section_body_overflows_treated_as_raw() {
        let mut bytes = raw_wasm_header();
        // Section 10 claiming length 0xff (255) but only 1 byte follows
        bytes.extend_from_slice(&[10, 0xff, 0x42]);
        assert_eq!(detect_component_kind(&bytes), ComponentKind::Raw);
    }

    #[test]
    fn decode_leb128_single_byte() {
        let bytes = [0x7f];
        let (val, pos) = decode_leb128(&bytes, 0).unwrap();
        assert_eq!(val, 0x7f);
        assert_eq!(pos, 1);
    }

    #[test]
    fn decode_leb128_two_bytes() {
        let bytes = [0x80, 0x01];
        let (val, pos) = decode_leb128(&bytes, 0).unwrap();
        assert_eq!(val, 128);
        assert_eq!(pos, 2);
    }

    #[test]
    fn decode_leb128_truncated_returns_none() {
        let bytes = [0x80];
        assert!(decode_leb128(&bytes, 0).is_none());
    }
}
