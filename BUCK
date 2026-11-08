load("//build_defs:integration_tests.bzl", "integration_tests")

rust_library(
    name = "phoebe_lib",
    crate = "phoebe",
    crate_root = "src/lib.rs",
    srcs = ["src/lib.rs", "src/project_session.rs"],
    edition = "2024",
    deps = [
        "//crates/capability:phoebe_capability",
        "//crates/core:phoebe_core",
        "//crates/frame:phoebe_frame",
        "//crates/native:phoebe_native",
        "//crates/package:phoebe_package",
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
    resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
)

genrule(
    name = "phoebe_dist",
    out = "phoebe_dist",
    cmd = "mkdir -p \"$OUT\" && cp \"$(location :phoebe)\" \"$OUT/phoebe\" && cp \"$(location //crates/runtime:runtime_archive)\" \"$OUT/libphoebe_runtime.a\"",
)

# Keep test targets in this package so their ../examples include paths resolve.
integration_tests(rust_test)
