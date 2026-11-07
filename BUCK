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
    resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
)

genrule(
    name = "phoebe_dist",
    out = "phoebe_dist",
    cmd = "mkdir -p \"$OUT\" && cp \"$(location :phoebe)\" \"$OUT/phoebe\" && cp \"$(location //crates/runtime:runtime_archive)\" \"$OUT/libphoebe_runtime.a\"",
)

rust_test(
    name = "source_traffic_tests",
    crate_root = "tests/source_traffic.rs",
    srcs = [
        "tests/source_traffic.rs",
        "examples/channel_exchange.phoebe",
        "examples/channel_exchange_helper.phoebe",
        "examples/channel_accumulator.phoebe",
        "examples/borrow_across_wait.phoebe",
        "examples/spawn_direct_from_poll.phoebe",
    ],
    edition = "2024",
    deps = [":phoebe_lib"],
    resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
)

rust_test(
    name = "source_spawn_tests",
    crate_root = "tests/source_spawn.rs",
    srcs = [
        "tests/source_spawn.rs",
        "examples/spawn_pair.phoebe",
    ],
    edition = "2024",
    deps = [":phoebe_lib"],
    resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
)
