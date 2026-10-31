// SPDX-License-Identifier: AGPL-3.0-or-later

fn main() {
    // Embed the application manifest (common controls v6, per-monitor DPI v2).
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=Cargo.toml"); // version/author → VERSIONINFO
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());

    // Embed VERSIONINFO (Explorer → Properties → Details). FileVersion,
    // ProductVersion, ProductName and FileDescription come from Cargo.toml.
    let authors = std::env::var("CARGO_PKG_AUTHORS").unwrap_or_default();
    let author = authors.split(':').next().unwrap_or("").split(" <").next().unwrap_or("");
    let mut res = winresource::WindowsResource::new();
    res.set("CompanyName", author);
    res.set("LegalCopyright", &format!("Copyright (C) 2026 {author} — licensed under the GNU AGPL v3 or later"));
    res.set("OriginalFilename", "nmi-notes.exe");
    res.set("InternalName", "nmi-notes");
    if let Err(e) = res.compile() {
        panic!("failed to embed version resource: {e}");
    }
}
