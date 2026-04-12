fn main() {
    // On macOS, embed Info.plist into the binary so that the TCC subsystem
    // recognises the daemon as a bundle-like executable with a bundle
    // identifier and privacy usage descriptions.  Without this, EventKit /
    // Contacts access requests are silently denied by macOS.
    //
    // After linking, the binary must be re-signed so the plist section is
    // bound to the code signature.  This happens automatically:
    //   - Dev builds: `cargo xtask build-daemon` (builds + codesigns)
    //   - Production: `scripts/macos/build-pkg.sh` (codesigns explicitly)
    #[cfg(target_os = "macos")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let plist = std::path::Path::new(&manifest_dir).join("Info.plist");
        println!("cargo:rerun-if-changed={}", plist.display());
        println!("cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}", plist.display());
    }
}
