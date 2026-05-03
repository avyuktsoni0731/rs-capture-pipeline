//! Output sinks: MP4 file (more formats later).

mod annexb;
mod mp4_file;

pub use annexb::{nal_units, nal_units_to_avcc_sample};
pub use mp4_file::Mp4H264File;
