//! Colours (0xRRGGBB). Dark, plain; theming comes with the app shell.

use mold_graph::PortType;

use crate::canvas::EvalState;

pub const BACKGROUND: u32 = 0x1b1e24;
pub const NODE: u32 = 0x2a2f38;
pub const HEADER: u32 = 0x343a45;
pub const BORDER: u32 = 0x4a5160;
pub const SELECTED: u32 = 0xf0c060;
pub const TEXT: u32 = 0xe6e9ef;
pub const MUTED: u32 = 0x9aa3b2;
pub const DIMMED: u32 = 0x5d6470;
pub const ACCEPTED: u32 = 0x4cd07d;
pub const REFUSED: u32 = 0xff5a5a;
pub const BADGE_TEXT: u32 = 0x10131a;

/// One colour per port type, shared by ports, edges and the dragged wire.
pub fn port_color(ty: PortType) -> u32 {
    match ty {
        PortType::Structure => 0x6aa8ff,
        PortType::Sequence => 0xc58bff,
        PortType::LigandSet => 0x52d6c4,
        PortType::Msa => 0xff9fd0,
        PortType::PoseSet => 0xffb454,
        PortType::Table => 0xa3d977,
        PortType::Scalar => 0xd0d4dc,
    }
}

pub fn eval_color(e: EvalState) -> u32 {
    match e {
        EvalState::Dirty => 0xe0b040,
        EvalState::Running => 0x5aa0ff,
        EvalState::Cached => 0x4cd07d,
        EvalState::Failed => 0xff5a5a,
    }
}
