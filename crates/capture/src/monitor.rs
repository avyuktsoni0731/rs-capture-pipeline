use anyhow::{anyhow, Context};
use windows::Graphics::DisplayId;
use windows::Graphics::Display::DisplayServices;

/// All connected displays as WinRT [`DisplayId`] values (order is OS-defined).
pub fn list_display_ids() -> anyhow::Result<Vec<DisplayId>> {
    let arr = DisplayServices::FindAll().context(
        "DisplayServices::FindAll (needs Win10 2004+ with Graphics_Display APIs)",
    )?;
    Ok(arr.as_slice().to_vec())
}

/// First display in [`list_display_ids`]. On a single-monitor machine this is the only screen.
pub fn default_display_id() -> anyhow::Result<DisplayId> {
    list_display_ids()?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no displays found"))
}
