def integration_tests(rust_test):
    rust_test(
        name = "generic_cli_tests",
        crate_root = "tests/generic_cli.rs",
        srcs = [
            "tests/generic_cli.rs",
            "examples/generic_call.phoebe",
            "examples/generic_project/phoebe.toml",
            "examples/generic_project/app.phoebe",
            "examples/generic_project/math.phoebe",
            "examples/generic_project/base.phoebe",
        ],
        edition = "2024",
        env = {"PHOEBE_TEST_COMPILER": "$(location :phoebe)"},
    )

    rust_test(
        name = "checked_program_tests",
        crate_root = "tests/checked_program.rs",
        srcs = ["tests/checked_program.rs", "examples/hello.phoebe", "examples/channel_exchange.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "build_session_cli_tests",
        crate_root = "tests/build_session_cli.rs",
        srcs = [
            "tests/build_session_cli.rs",
            "examples/multi_module/phoebe.toml",
            "examples/multi_module/app.phoebe",
            "examples/multi_module/math.phoebe",
            "examples/generic_project/phoebe.toml",
            "examples/generic_project/app.phoebe",
            "examples/generic_project/math.phoebe",
            "examples/generic_project/base.phoebe",
        ],
        edition = "2024",
        env = {
            "PHOEBE_TEST_COMPILER": "$(location :phoebe)",
            "PHOEBE_RUNTIME_LIB": "$(location //crates/runtime:runtime_archive)",
        },
    )

    rust_test(
        name = "source_scopes_tests",
        crate_root = "tests/source_scopes.rs",
        srcs = ["tests/source_scopes.rs", "examples/scoped_spawn.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_traffic_tests",
        crate_root = "tests/source_traffic.rs",
        srcs = [
            "tests/source_traffic.rs",
            "examples/channel_exchange.phoebe",
            "examples/channel_exchange_helper.phoebe",
            "examples/channel_accumulator.phoebe",
            "examples/bench_channel_throughput.phoebe",
            "examples/borrow_across_wait.phoebe",
            "examples/wide_after_wait.phoebe",
            "examples/spawn_direct_from_poll.phoebe",
        ],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "compiler_tests",
        crate_root = "tests/compiler.rs",
        srcs = [
            "tests/compiler.rs",
            "examples/boolean_short_circuit.phoebe",
            "examples/borrow.phoebe",
            "examples/copy.phoebe",
            "examples/nested_result.phoebe",
            "examples/option.phoebe",
            "examples/payload.phoebe",
            "examples/product.phoebe",
            "examples/result_in_data.phoebe",
            "examples/try.phoebe",
            "examples/variant.phoebe",
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

    rust_test(
        name = "source_with_tests",
        crate_root = "tests/source_with.rs",
        srcs = [
            "tests/source_with.rs",
            "examples/with_channel.phoebe",
            "examples/with_result.phoebe",
            "examples/with_wait.phoebe",
        ],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_with_group_tests",
        crate_root = "tests/source_with_group.rs",
        srcs = ["tests/source_with_group.rs", "examples/with_pair.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_yield_tests",
        crate_root = "tests/source_yield.rs",
        srcs = [
            "tests/source_yield.rs",
            "examples/loop.phoebe",
            "examples/yield_loop.phoebe",
            "examples/with_wait.phoebe",
        ],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_unit_tests",
        crate_root = "tests/source_unit.rs",
        srcs = ["tests/source_unit.rs", "examples/unit_tasks.phoebe", "examples/unit_data.phoebe", "examples/unit_place.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_fork_tests",
        crate_root = "tests/source_fork.rs",
        srcs = [
            "tests/source_fork.rs",
            "examples/fork_channel.phoebe",
        ],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "project_tests",
        crate_root = "tests/project.rs",
        srcs = [
            "tests/project.rs",
            "examples/multi_module/phoebe.toml",
            "examples/multi_module/app.phoebe",
            "examples/multi_module/math.phoebe",
            "examples/request_exchange.phoebe",
            "examples/channel_exchange.phoebe",
            "examples/package_graph/app/phoebe.toml",
            "examples/package_graph/app/phoebe.lock",
            "examples/package_graph/app/app.phoebe",
            "examples/package_graph/math/phoebe.toml",
            "examples/package_graph/math/api.phoebe",
            "examples/package_graph/math/helper.phoebe",
            "examples/package_graph/util/phoebe.toml",
            "examples/package_graph/util/api.phoebe",
        ],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "generic_project_tests",
        crate_root = "tests/generic_project.rs",
        srcs = [
            "tests/generic_project.rs",
            "examples/generic_project/phoebe.toml",
            "examples/generic_project/app.phoebe",
            "examples/generic_project/math.phoebe",
            "examples/generic_project/base.phoebe",
            "examples/channel_exchange.phoebe",
        ],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    # Keep every Cargo integration suite as an independent Buck2 target.
    rust_test(
        name = "capability_flow_tests",
        crate_root = "tests/capability_flow.rs",
        srcs = ["tests/capability_flow.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "core_channels_tests",
        crate_root = "tests/core_channels.rs",
        srcs = ["tests/core_channels.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "core_spawn_tests",
        crate_root = "tests/core_spawn.rs",
        srcs = ["tests/core_spawn.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "core_scopes_tests",
        crate_root = "tests/core_scopes.rs",
        srcs = ["tests/core_scopes.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "cost_acceptance_tests",
        crate_root = "tests/cost_acceptance.rs",
        srcs = ["tests/cost_acceptance.rs", "examples/cost/identity.phoebe", "examples/cost/product_identity.phoebe", "examples/cost/empty_scope.phoebe", "examples/cost/unit_identity.phoebe", "examples/spawn_pair.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "diagnostics_tests",
        crate_root = "tests/diagnostics.rs",
        srcs = [
            "tests/diagnostics.rs",
            "examples/boolean_short_circuit.phoebe",
            "examples/spawn_pair.phoebe",
            "examples/nested_variant.phoebe",
            "examples/try.phoebe",
        ],
        edition = "2024",
        deps = [":phoebe_lib"],
    )

    rust_test(
        name = "frame_layout_tests",
        crate_root = "tests/frame_layout.rs",
        srcs = ["tests/frame_layout.rs", "examples/channel_exchange.phoebe", "examples/spawn_close.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "nested_variant_tests",
        crate_root = "tests/nested_variant.rs",
        srcs = ["tests/nested_variant.rs", "examples/nested_variant.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "qualified_calls_tests",
        crate_root = "tests/qualified_calls.rs",
        srcs = ["tests/qualified_calls.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "reply_runtime_tests",
        crate_root = "tests/reply_runtime.rs",
        srcs = ["tests/reply_runtime.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "core_replies_tests",
        crate_root = "tests/core_replies.rs",
        srcs = ["tests/core_replies.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "core_requests_tests",
        crate_root = "tests/core_requests.rs",
        srcs = ["tests/core_requests.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "core_product_unpack_tests",
        crate_root = "tests/core_product_unpack.rs",
        srcs = ["tests/core_product_unpack.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_product_unpack_tests",
        crate_root = "tests/source_product_unpack.rs",
        srcs = ["tests/source_product_unpack.rs", "examples/product_unpack.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_capability_sums_tests",
        crate_root = "tests/source_capability_sums.rs",
        srcs = [
            "tests/source_capability_sums.rs",
            "examples/capability_option.phoebe",
            "examples/capability_result.phoebe",
            "examples/capability_sum_close.phoebe",
            "examples/capability_sum_cleanup.phoebe",
            "examples/capability_sum_product.phoebe",
            "examples/capability_nested_sum.phoebe",
        ],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_capability_products_tests",
        crate_root = "tests/source_capability_products.rs",
        srcs = ["tests/source_capability_products.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_capability_spawn_tests",
        crate_root = "tests/source_capability_spawn.rs",
        srcs = ["tests/source_capability_spawn.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_with_product_tests",
        crate_root = "tests/source_with_product.rs",
        srcs = ["tests/source_with_product.rs", "examples/with_product.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_replies_tests",
        crate_root = "tests/source_replies.rs",
        srcs = ["tests/source_replies.rs", "examples/reply_exchange.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_requests_tests",
        crate_root = "tests/source_requests.rs",
        srcs = ["tests/source_requests.rs", "examples/request_exchange.phoebe", "examples/request_backpressure.phoebe"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "runtime_tests",
        crate_root = "tests/runtime.rs",
        srcs = ["tests/runtime.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "runtime_owned_errors_tests",
        crate_root = "tests/runtime_owned_errors.rs",
        srcs = ["tests/runtime_owned_errors.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )

    rust_test(
        name = "source_channels_tests",
        crate_root = "tests/source_channels.rs",
        srcs = ["tests/source_channels.rs"],
        edition = "2024",
        deps = [":phoebe_lib"],
        resources = {"libphoebe_runtime.a": "//crates/runtime:runtime_archive"},
    )
