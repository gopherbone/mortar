// SPDX-License-Identifier: GPL-2.0-or-later
//! Line icons from the Mortar plan mockups (24x24 viewBox, stroked with
//! `currentColor`). Sessions name them in config by their string id.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Icon {
    Wifi,
    Bat,
    Usb,
    Cable,
    File,
    Skip,
    Term,
    Folder,
    Pad,
    Gear,
    Link,
    Check,
    Lock,
    Power,
}

impl Icon {
    pub fn svg(self) -> &'static str {
        match self {
            Icon::Wifi => r#"<path d="M2 9a15 15 0 0 1 20 0M5.5 12.5a10 10 0 0 1 13 0M9 16a5 5 0 0 1 6 0" stroke-linecap="round"/><circle cx="12" cy="19.5" r="1" fill="currentColor"/>"#,
            Icon::Bat => r#"<rect x="2" y="7" width="18" height="10" rx="2"/><path d="M22 10.5v3" stroke-linecap="round"/><rect x="4.5" y="9.5" width="11" height="5" fill="currentColor" stroke="none"/>"#,
            Icon::Usb => r#"<path d="M12 2v16M12 18a2 2 0 1 0 0 .01M8 7l4-4 4 4M7 11v2l5 3M17 9v3l-5 3" stroke-linecap="round" stroke-linejoin="round"/>"#,
            Icon::Cable => r#"<path d="M7 3v5M17 3v5M5 8h14v3a7 7 0 0 1-14 0zM12 18v4"/>"#,
            Icon::File => r#"<path d="M14 3H6v18h12V7zM14 3v4h4M9 12h6M9 16h6"/>"#,
            Icon::Skip => r#"<path d="M5 5l8 7-8 7zM17 5v14"/>"#,
            Icon::Term => r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 9l3 3-3 3M12 15h5"/>"#,
            Icon::Folder => r#"<path d="M3 6h6l2 2h10v11H3z"/>"#,
            Icon::Pad => r#"<rect x="2" y="7" width="20" height="11" rx="5"/><path d="M7 10.5v4M5 12.5h4M15.5 12h.01M18 14h.01"/>"#,
            Icon::Gear => r#"<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1"/>"#,
            Icon::Link => r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>"#,
            Icon::Check => r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#,
            Icon::Lock => r#"<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>"#,
            Icon::Power => r#"<path d="M12 3v8M7.5 6.5a7 7 0 1 0 9 0"/>"#,
        }
    }
}
