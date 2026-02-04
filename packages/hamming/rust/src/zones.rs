//! Codebook address zone definitions.
//!
//! The 4,096-entry codebook is divided into zones, each serving a specific
//! semantic purpose in the A2UI protocol.

use std::ops::RangeInclusive;

/// A named zone within the codebook address space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    pub name: &'static str,
    pub range: RangeInclusive<u32>,
    pub description: &'static str,
}

impl Zone {
    pub fn contains(&self, address: u32) -> bool {
        self.range.contains(&address)
    }

    pub fn capacity(&self) -> u32 {
        self.range.end() - self.range.start() + 1
    }
}

/// Protocol control: handshake, versioning, frame types
pub const ZONE_PROTOCOL: Zone = Zone {
    name: "protocol",
    range: 0x000..=0x01F,
    description: "Frame types, handshake, versioning, control signals",
};

/// A2UI message types and envelopes
pub const ZONE_MESSAGES: Zone = Zone {
    name: "messages",
    range: 0x020..=0x07F,
    description: "A2UI message types: createSurface, updateComponents, etc.",
};

/// Standard catalog components
pub const ZONE_COMPONENTS: Zone = Zone {
    name: "components",
    range: 0x080..=0x1FF,
    description: "Standard catalog: Text, Button, Card, Row, Column, etc.",
};

/// Component properties and values
pub const ZONE_PROPERTIES: Zone = Zone {
    name: "properties",
    range: 0x200..=0x3FF,
    description: "Component properties, attribute values, common defaults",
};

/// Styles, themes, layout directives
pub const ZONE_STYLES: Zone = Zone {
    name: "styles",
    range: 0x400..=0x5FF,
    description: "Theme entries, layout modes, colors, typography, spacing",
};

/// Data model operations
pub const ZONE_DATA_OPS: Zone = Zone {
    name: "data_ops",
    range: 0x600..=0x7FF,
    description: "Data model paths, JSON Pointer bindings, state updates",
};

/// User actions and events
pub const ZONE_ACTIONS: Zone = Zone {
    name: "actions",
    range: 0x800..=0x9FF,
    description: "User interactions: clicks, inputs, scroll, navigation",
};

/// Pre-composed multi-operation entries
pub const ZONE_COMPOSITES: Zone = Zone {
    name: "composites",
    range: 0xA00..=0xBFF,
    description: "Pre-computed XOR compositions for common operation sequences",
};

/// Dynamic/learned patterns
pub const ZONE_DYNAMIC: Zone = Zone {
    name: "dynamic",
    range: 0xC00..=0xEFF,
    description: "Session-learned patterns, usage-optimized entries, user preferences",
};

/// Cognitive operations (ladybug-rs NARS integration)
pub const ZONE_COGNITIVE: Zone = Zone {
    name: "cognitive",
    range: 0xF00..=0xFFF,
    description: "NARS inference, semantic reasoning, codebook self-management",
};

/// All zones in address order.
pub const ALL_ZONES: &[&Zone] = &[
    &ZONE_PROTOCOL,
    &ZONE_MESSAGES,
    &ZONE_COMPONENTS,
    &ZONE_PROPERTIES,
    &ZONE_STYLES,
    &ZONE_DATA_OPS,
    &ZONE_ACTIONS,
    &ZONE_COMPOSITES,
    &ZONE_DYNAMIC,
    &ZONE_COGNITIVE,
];

/// Resolve which zone an address belongs to.
pub fn zone_for_address(address: u32) -> Option<&'static Zone> {
    ALL_ZONES.iter().find(|z| z.contains(address)).copied()
}

// ─────────────────────────────────────────────────────────
// Well-known addresses within each zone
// ─────────────────────────────────────────────────────────

/// Protocol zone well-known addresses
pub mod protocol {
    pub const HANDSHAKE_INIT: u32 = 0x000;
    pub const HANDSHAKE_ACK: u32 = 0x001;
    pub const HEARTBEAT: u32 = 0x002;
    pub const CODEBOOK_SYNC: u32 = 0x003;
    pub const SESSION_CLOSE: u32 = 0x004;
    pub const ERROR: u32 = 0x005;
    pub const VERSION_ANNOUNCE: u32 = 0x006;
    pub const CAPABILITY_QUERY: u32 = 0x007;
    pub const CAPABILITY_RESPONSE: u32 = 0x008;
    pub const NOP: u32 = 0x01F;
}

/// Message zone well-known addresses
pub mod messages {
    pub const CREATE_SURFACE: u32 = 0x020;
    pub const UPDATE_COMPONENTS: u32 = 0x021;
    pub const UPDATE_DATA_MODEL: u32 = 0x022;
    pub const DELETE_SURFACE: u32 = 0x023;
    pub const ATTACH_DATA_MODEL: u32 = 0x024;
    pub const BEGIN_RENDERING: u32 = 0x025;   // v0.8 compat
    pub const SURFACE_UPDATE: u32 = 0x026;     // v0.8 compat
}

/// Component zone well-known addresses
pub mod components {
    pub const TEXT: u32 = 0x080;
    pub const BUTTON: u32 = 0x081;
    pub const CARD: u32 = 0x082;
    pub const ROW: u32 = 0x083;
    pub const COLUMN: u32 = 0x084;
    pub const IMAGE: u32 = 0x085;
    pub const TEXT_FIELD: u32 = 0x086;
    pub const SLIDER: u32 = 0x087;
    pub const CHECKBOX: u32 = 0x088;
    pub const MODAL: u32 = 0x089;
    pub const TABS: u32 = 0x08A;
    pub const LIST: u32 = 0x08B;
    pub const DIVIDER: u32 = 0x08C;
    pub const ICON: u32 = 0x08D;
    pub const VIDEO: u32 = 0x08E;
    pub const AUDIO: u32 = 0x08F;
    pub const DATE_TIME_INPUT: u32 = 0x090;
    pub const MULTIPLE_CHOICE: u32 = 0x091;
    pub const MARKDOWN: u32 = 0x092;
    pub const PROGRESS: u32 = 0x093;
}

/// Style zone well-known addresses
pub mod styles {
    pub const THEME_LIGHT: u32 = 0x400;
    pub const THEME_DARK: u32 = 0x401;
    pub const LAYOUT_FLEX_ROW: u32 = 0x410;
    pub const LAYOUT_FLEX_COLUMN: u32 = 0x411;
    pub const LAYOUT_GRID: u32 = 0x412;
    pub const SPACING_SMALL: u32 = 0x420;
    pub const SPACING_MEDIUM: u32 = 0x421;
    pub const SPACING_LARGE: u32 = 0x422;
    pub const COLOR_PRIMARY: u32 = 0x430;
    pub const COLOR_SECONDARY: u32 = 0x431;
    pub const COLOR_ERROR: u32 = 0x432;
    pub const COLOR_SUCCESS: u32 = 0x433;
    pub const FONT_HEADING: u32 = 0x440;
    pub const FONT_BODY: u32 = 0x441;
    pub const FONT_MONO: u32 = 0x442;
}

/// Action zone well-known addresses
pub mod actions {
    pub const CLICK: u32 = 0x800;
    pub const TEXT_INPUT: u32 = 0x801;
    pub const SCROLL: u32 = 0x802;
    pub const FOCUS: u32 = 0x803;
    pub const BLUR: u32 = 0x804;
    pub const SUBMIT: u32 = 0x805;
    pub const NAVIGATE: u32 = 0x806;
    pub const SELECT: u32 = 0x807;
    pub const DRAG: u32 = 0x808;
    pub const DROP: u32 = 0x809;
}

/// Cognitive zone well-known addresses
pub mod cognitive {
    pub const NARS_QUERY: u32 = 0xF00;
    pub const NARS_ASSERT: u32 = 0xF01;
    pub const NARS_INFER: u32 = 0xF02;
    pub const SEMANTIC_SEARCH: u32 = 0xF03;
    pub const OPTIMIZE_REQUEST: u32 = 0xF04;
    pub const PATTERN_LEARNED: u32 = 0xF05;
    pub const PREDICTIVE_PREFETCH: u32 = 0xF06;
    pub const CODEBOOK_REORGANIZE: u32 = 0xF07;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zone_lookup() {
        assert_eq!(zone_for_address(0x000).unwrap().name, "protocol");
        assert_eq!(zone_for_address(0x020).unwrap().name, "messages");
        assert_eq!(zone_for_address(0x080).unwrap().name, "components");
        assert_eq!(zone_for_address(0x400).unwrap().name, "styles");
        assert_eq!(zone_for_address(0x800).unwrap().name, "actions");
        assert_eq!(zone_for_address(0xF00).unwrap().name, "cognitive");
    }

    #[test]
    fn test_zone_capacity() {
        assert_eq!(ZONE_PROTOCOL.capacity(), 32);
        assert_eq!(ZONE_COMPONENTS.capacity(), 384);
        assert_eq!(ZONE_DYNAMIC.capacity(), 768);
    }

    #[test]
    fn test_total_capacity() {
        let total: u32 = ALL_ZONES.iter().map(|z| z.capacity()).sum();
        assert_eq!(total, 4096);
    }
}
