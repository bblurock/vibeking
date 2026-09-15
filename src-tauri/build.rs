fn main() {
    // fluidaudio-rs links against the Swift Concurrency runtime. Without an
    // explicit rpath, the bare dev binary (cargo run / pnpm tauri dev) crashes
    // at startup with
    //   dyld: Library not loaded: @rpath/libswift_Concurrency.dylib
    //
    // The fix is to add `/usr/lib/swift` as an rpath: on macOS 12+ the Swift
    // standard libraries live inside the dyld shared cache (not on disk as
    // files), but dyld will resolve `@rpath/libswift_*.dylib` from the cache
    // when the rpath path matches. Crucially, do NOT add Xcode's
    // `swift-5.5/macosx` directory as an rpath — fluidaudio-rs's bundled
    // Swift package independently references /usr/lib/swift, so mixing in
    // Xcode's disk copy causes both versions to load and yields duplicate
    // Swift class warnings ("This may cause spurious casting failures and
    // mysterious crashes") followed by an actual crash.
    //
    // The Tauri bundled .app gets the equivalent rpath baked in via
    // codesigning; this only affects unbundled dev runs.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    tauri_build::build()
}
