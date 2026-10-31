// SPDX-License-Identifier: AGPL-3.0-or-later

//! Version, author, and license info (from Cargo.toml).

pub const NAME: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const AUTHORS: &str = env!("CARGO_PKG_AUTHORS");
pub const LICENSE: &str = env!("CARGO_PKG_LICENSE");

/// Multi-line notice for `--version` and the tray's About box.
pub fn text() -> String {
    let authors = AUTHORS.replace(':', ", ");
    format!(
        "{NAME} {VERSION}\n\
         Copyright (C) 2026 {authors}\n\
         License: {LICENSE} (GNU Affero General Public License v3 or later)\n\n\
         This program is free software: you can redistribute it and/or modify it under the terms of the \
         GNU Affero General Public License as published by the Free Software Foundation, either version 3 \
         of the License, or (at your option) any later version.\n\n\
         This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without \
         even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU \
         Affero General Public License for more details: <https://www.gnu.org/licenses/agpl-3.0.html>."
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn text_names_version_and_license() {
        let t = super::text();
        assert!(t.starts_with(&format!("nmi-notes {}", super::VERSION)));
        assert!(t.contains("AGPL-3.0-or-later"));
    }
}
