const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    // Build as a shared library (.so)
    const lib = b.addSharedLibrary(.{
        .name = "wifi_scan",
        .root_source_file = b.path("src/scanner.zig"),
        .target = target,
        .optimize = optimize,
    });

    // We're using POSIX APIs
    lib.linkLibC();

    b.installArtifact(lib);

    // Also build a test executable for standalone testing
    const test_exe = b.addTest(.{
        .root_source_file = b.path("src/scanner.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_exe.linkLibC();

    const run_tests = b.addRunArtifact(test_exe);
    const test_step = b.step("test", "Run unit tests");
    test_step.dependOn(&run_tests.step);
}