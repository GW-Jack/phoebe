rust_library(
    name = "phoebe_lib",
    crate = "phoebe",
    crate_root = "src/lib.rs",
    srcs = ["src/lib.rs"],
    edition = "2024",
    deps = [
        "//crates/capability:phoebe_capability",
        "//crates/core:phoebe_core",
        "//crates/frame:phoebe_frame",
        "//crates/native:phoebe_native",
        "//crates/project:phoebe_project",
        "//crates/runtime:phoebe_runtime",
        "//crates/syntax:phoebe_syntax",
    ],
    visibility = ["PUBLIC"],
)

rust_binary(
    name = "phoebe",
    crate_root = "src/main.rs",
    srcs = ["src/main.rs"],
    edition = "2024",
    deps = [":phoebe_lib"],
)
