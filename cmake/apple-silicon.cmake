# Do not compile Whisper's CPU backend for the build machine's specific chip.
# This also avoids GGML's i8mm feature-probe failure on GitHub's M1 runners.
# Metal and Accelerate remain enabled by the app's normal build configuration.
if(APPLE AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(arm64|aarch64)$")
    set(GGML_NATIVE OFF CACHE BOOL "Build a portable Apple Silicon CPU backend" FORCE)
endif()
