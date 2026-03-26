// ============================================================
// Signal Observatory — nl80211 WiFi Scanner (Zig → .so → Rust FFI)
// ============================================================
// This library talks directly to the Linux kernel via Netlink
// sockets to scan for nearby WiFi networks. It exposes a C ABI
// so Rust can call it through FFI.
//
// v2.0.0 — Auto-detection, BSS status, cleaned output,
//          capability export, unit tests.
// ============================================================

const std = @import("std");
const os = std.os;
const mem = std.mem;

// ──────────────────────────────────────────────
// SECTION 1: Constants from Linux kernel headers
// ──────────────────────────────────────────────
// These come from:
//   linux/netlink.h
//   linux/genetlink.h
//   linux/nl80211.h

// Netlink protocol families
const NETLINK_GENERIC = 16;

// Netlink message flags
const NLM_F_REQUEST = 0x0001;
const NLM_F_MULTI = 0x0002;
const NLM_F_ACK = 0x0004;
const NLM_F_DUMP = 0x0300;

// Netlink message types
const NLMSG_NOOP = 0x1;
const NLMSG_ERROR = 0x2;
const NLMSG_DONE = 0x3;

// Generic Netlink
const GENL_ID_CTRL = 0x10;
const CTRL_CMD_GETFAMILY = 3;
const CTRL_ATTR_FAMILY_ID = 1;
const CTRL_ATTR_FAMILY_NAME = 2;

// nl80211 commands
const NL80211_CMD_GET_SCAN = 32;
const NL80211_CMD_TRIGGER_SCAN = 33;

// nl80211 attributes
const NL80211_ATTR_IFINDEX = 3;
const NL80211_ATTR_SCAN_FREQUENCIES = 44;
const NL80211_ATTR_BSS = 47;

// BSS (Basic Service Set) sub-attributes — nested inside NL80211_ATTR_BSS
const NL80211_BSS_BSSID = 1;
const NL80211_BSS_FREQUENCY = 2;
const NL80211_BSS_BEACON_INTERVAL = 3;
const NL80211_BSS_CAPABILITY = 4;
const NL80211_BSS_INFORMATION_ELEMENTS = 6;
const NL80211_BSS_SIGNAL_MBM = 7;
const NL80211_BSS_STATUS = 9;

// BSS status values
const NL80211_BSS_STATUS_AUTHENTICATED = 0;
const NL80211_BSS_STATUS_ASSOCIATED = 1;
const NL80211_BSS_STATUS_IBSS_JOINED = 2;

// Socket address for Netlink
const sockaddr_nl = extern struct {
    family: u16 = std.os.linux.AF.NETLINK,
    pad: u16 = 0,
    pid: u32 = 0, // 0 = kernel
    groups: u32 = 0,
};

// ──────────────────────────────────────────────
// SECTION 2: C ABI Types (the contract with Rust)
// ──────────────────────────────────────────────

const MAX_NETWORKS = 128;

// Security type enum (matches Rust side)
const SecurityType = enum(u8) {
    Open = 0,
    WEP = 1,
    WPA = 2,
    WPA2 = 3,
    WPA3 = 4,
};

// Capability flags (bitmask)
const CAP_SCAN_TRIGGER = 0x01;
const CAP_CACHED_RESULTS = 0x02;
const CAP_BSS_STATUS = 0x04;
const CAP_BEACON_INTERVAL = 0x08;
const CAP_AUTO_IFACE = 0x10;
const CAP_SECURITY_DETECT = 0x20;

// One WiFi network — C ABI compatible
const CWifiNetwork = extern struct {
    ssid: [33]u8, // 32 chars + null terminator
    bssid: [6]u8, // MAC address bytes
    signal_dbm: i32, // signal in dBm (e.g., -45)
    frequency: u32, // in MHz (e.g., 2437)
    channel: u8,
    security: u8, // SecurityType enum value
    bss_status: u8, // 0=none, 1=authenticated, 2=associated, 3=IBSS
    beacon_interval: u16, // in TU (1024 μs)
    _pad: [1]u8 = .{0}, // alignment padding
};

// Result of a scan — C ABI compatible
const CScanResult = extern struct {
    networks: [*]CWifiNetwork, // pointer to array
    count: u32,
    error_code: i32, // 0 = success, negative = errno
};

// Verbosity control
var g_verbose: bool = false;

fn debugLog(comptime fmt: []const u8, args: anytype) void {
    if (g_verbose) {
        std.debug.print(fmt, args);
    }
}

// ──────────────────────────────────────────────
// SECTION 3: Netlink Message Building
// ──────────────────────────────────────────────

fn nlmsg_align(len: u32) u32 {
    return (len + 3) & ~@as(u32, 3);
}

const NlMsgHeader = extern struct {
    len: u32,
    type_: u16,
    flags: u16,
    seq: u32,
    pid: u32,
};

const GenlMsgHeader = extern struct {
    cmd: u8,
    version: u8,
    reserved: u16 = 0,
};

const NlAttrHeader = extern struct {
    len: u16,
    type_: u16,
};

const NlMsgBuilder = struct {
    buf: []u8,
    pos: usize,

    fn init(buf: []u8) NlMsgBuilder {
        return .{ .buf = buf, .pos = 0 };
    }

    fn putNlmsghdr(self: *NlMsgBuilder, msg_type: u16, flags: u16, seq: u32) !*NlMsgHeader {
        const size = @sizeOf(NlMsgHeader);
        if (self.pos + size > self.buf.len) return error.BufferFull;
        const hdr: *NlMsgHeader = @ptrCast(@alignCast(self.buf[self.pos..].ptr));
        hdr.* = .{
            .len = 0,
            .type_ = msg_type,
            .flags = flags,
            .seq = seq,
            .pid = 0,
        };
        self.pos += size;
        return hdr;
    }

    fn putGenlmsghdr(self: *NlMsgBuilder, cmd: u8, version: u8) !void {
        const size = @sizeOf(GenlMsgHeader);
        if (self.pos + size > self.buf.len) return error.BufferFull;
        const hdr: *GenlMsgHeader = @ptrCast(@alignCast(self.buf[self.pos..].ptr));
        hdr.* = .{
            .cmd = cmd,
            .version = version,
        };
        self.pos += size;
    }

    fn putAttrU32(self: *NlMsgBuilder, attr_type: u16, value: u32) !void {
        const hdr_size = @sizeOf(NlAttrHeader);
        const payload_size: u16 = 4;
        const total = nlmsg_align(hdr_size + payload_size);
        if (self.pos + total > self.buf.len) return error.BufferFull;

        const attr: *NlAttrHeader = @ptrCast(@alignCast(self.buf[self.pos..].ptr));
        attr.* = .{ .len = hdr_size + payload_size, .type_ = attr_type };

        const val_ptr: *u32 = @ptrCast(@alignCast(self.buf[self.pos + hdr_size ..].ptr));
        val_ptr.* = value;

        self.pos += total;
    }

    fn putAttrStr(self: *NlMsgBuilder, attr_type: u16, str: []const u8) !void {
        const hdr_size = @sizeOf(NlAttrHeader);
        const payload_len: u16 = @intCast(str.len + 1);
        const total = nlmsg_align(hdr_size + payload_len);
        if (self.pos + total > self.buf.len) return error.BufferFull;

        const attr: *NlAttrHeader = @ptrCast(@alignCast(self.buf[self.pos..].ptr));
        attr.* = .{ .len = hdr_size + payload_len, .type_ = attr_type };

        const dest = self.buf[self.pos + hdr_size .. self.pos + hdr_size + str.len];
        @memcpy(dest, str);
        self.buf[self.pos + hdr_size + str.len] = 0;

        self.pos += total;
    }

    fn finalize(self: *NlMsgBuilder, hdr: *NlMsgHeader) void {
        hdr.len = @intCast(self.pos);
    }
};

// ──────────────────────────────────────────────
// SECTION 4: Netlink Socket Wrapper
// ──────────────────────────────────────────────

const NlSocket = struct {
    fd: std.posix.fd_t,
    seq: u32 = 1,

    fn open() !NlSocket {
        const fd = try std.posix.socket(
            std.os.linux.AF.NETLINK,
            std.os.linux.SOCK.RAW | std.os.linux.SOCK.CLOEXEC,
            NETLINK_GENERIC,
        );
        errdefer std.posix.close(fd);

        const addr = sockaddr_nl{};
        try std.posix.bind(fd, @ptrCast(&addr), @sizeOf(sockaddr_nl));

        return .{ .fd = fd };
    }

    fn close(self: *NlSocket) void {
        std.posix.close(self.fd);
    }

    fn send(self: *NlSocket, buf: []const u8) !void {
        const addr = sockaddr_nl{};
        _ = try std.posix.sendto(
            self.fd,
            buf,
            0,
            @ptrCast(&addr),
            @sizeOf(sockaddr_nl),
        );
    }

    fn recv(self: *NlSocket, buf: []u8) !usize {
        return try std.posix.recv(self.fd, buf, 0);
    }

    fn nextSeq(self: *NlSocket) u32 {
        const s = self.seq;
        self.seq += 1;
        return s;
    }
};

// ──────────────────────────────────────────────
// SECTION 5: nl80211 Family ID Resolution
// ──────────────────────────────────────────────

fn resolveNl80211Family(sock: *NlSocket) !u16 {
    var buf: [4096]u8 align(4) = undefined;
    var builder = NlMsgBuilder.init(&buf);

    const seq = sock.nextSeq();
    const hdr = try builder.putNlmsghdr(GENL_ID_CTRL, NLM_F_REQUEST | NLM_F_ACK, seq);
    try builder.putGenlmsghdr(CTRL_CMD_GETFAMILY, 1);
    try builder.putAttrStr(CTRL_ATTR_FAMILY_NAME, "nl80211");
    builder.finalize(hdr);

    try sock.send(buf[0..hdr.len]);

    var recv_buf: [4096]u8 align(4) = undefined;
    const n = try sock.recv(&recv_buf);
    if (n < @sizeOf(NlMsgHeader)) return error.ShortRead;

    const resp_hdr: *const NlMsgHeader = @ptrCast(@alignCast(&recv_buf));
    if (resp_hdr.type_ == NLMSG_ERROR) {
        const err_code: *const i32 = @ptrCast(@alignCast(recv_buf[@sizeOf(NlMsgHeader)..]));
        if (err_code.* != 0) return error.NlError;
    }

    const attr_offset = @sizeOf(NlMsgHeader) + @sizeOf(GenlMsgHeader);
    var pos: usize = attr_offset;

    while (pos + @sizeOf(NlAttrHeader) <= n) {
        const attr: *const NlAttrHeader = @ptrCast(@alignCast(recv_buf[pos..].ptr));
        if (attr.len < @sizeOf(NlAttrHeader)) break;

        if (attr.type_ == CTRL_ATTR_FAMILY_ID) {
            const id: *const u16 = @ptrCast(@alignCast(recv_buf[pos + @sizeOf(NlAttrHeader) ..].ptr));
            return id.*;
        }

        pos += nlmsg_align(attr.len);
    }

    return error.FamilyNotFound;
}

// ──────────────────────────────────────────────
// SECTION 6: Get WiFi Interface Index
// ──────────────────────────────────────────────

fn getIfIndex(name: []const u8) !i32 {
    const fd = try std.posix.socket(
        std.os.linux.AF.INET,
        std.os.linux.SOCK.DGRAM,
        0,
    );
    defer std.posix.close(fd);

    var ifr: [40]u8 = [_]u8{0} ** 40;
    const copy_len = @min(name.len, 15);
    @memcpy(ifr[0..copy_len], name[0..copy_len]);

    const SIOCGIFINDEX = 0x8933;
    const result = std.os.linux.ioctl(fd, SIOCGIFINDEX, @intFromPtr(&ifr));
    if (result < 0) return error.IoctlFailed;

    const idx: *const i32 = @ptrCast(@alignCast(ifr[16..20].ptr));
    return idx.*;
}

// ──────────────────────────────────────────────
// SECTION 6b: Auto-detect WiFi Interface
// ──────────────────────────────────────────────
// Scan /sys/class/net/*/wireless to find a valid WiFi interface.

fn autoDetectWifiInterface(out_name: *[16]u8) bool {
    var dir = std.fs.openDirAbsolute("/sys/class/net", .{ .iterate = true }) catch return false;
    defer dir.close();

    var iter = dir.iterate();
    while (iter.next() catch null) |entry| {
        if (entry.kind != .directory and entry.kind != .sym_link) continue;

        // Check if /sys/class/net/<name>/wireless exists
        var path_buf: [256]u8 = undefined;
        const path = std.fmt.bufPrint(&path_buf, "/sys/class/net/{s}/wireless", .{entry.name}) catch continue;

        const stat = std.fs.cwd().statFile(path) catch continue;
        _ = stat;

        // Found a wireless interface!
        const name_len = @min(entry.name.len, 15);
        @memcpy(out_name[0..name_len], entry.name[0..name_len]);
        out_name[name_len] = 0;
        debugLog("Auto-detected WiFi interface: {s}\n", .{entry.name});
        return true;
    }
    return false;
}

// ──────────────────────────────────────────────
// SECTION 7: Trigger Scan + Get Results
// ──────────────────────────────────────────────

fn triggerScan(sock: *NlSocket, family_id: u16, if_index: i32) !void {
    var buf: [4096]u8 align(4) = undefined;
    var builder = NlMsgBuilder.init(&buf);

    const seq = sock.nextSeq();
    const hdr = try builder.putNlmsghdr(family_id, NLM_F_REQUEST | NLM_F_ACK, seq);
    try builder.putGenlmsghdr(NL80211_CMD_TRIGGER_SCAN, 0);
    try builder.putAttrU32(NL80211_ATTR_IFINDEX, @bitCast(if_index));
    builder.finalize(hdr);

    try sock.send(buf[0..hdr.len]);

    var recv_buf: [4096]u8 align(4) = undefined;
    const n = try sock.recv(&recv_buf);

    var msg_pos: usize = 0;
    while (msg_pos + @sizeOf(NlMsgHeader) <= n) {
        const msg_hdr: *const NlMsgHeader = @ptrCast(@alignCast(recv_buf[msg_pos..].ptr));
        if (msg_hdr.len < @sizeOf(NlMsgHeader) or msg_hdr.len > n - msg_pos) break;

        if (msg_hdr.type_ == NLMSG_ERROR) {
            const err_code_ptr: *const i32 = @ptrCast(@alignCast(recv_buf[msg_pos + @sizeOf(NlMsgHeader) ..][0..4]));
            return switch (err_code_ptr.*) {
                0 => {},
                -1 => error.PermissionDenied,
                else => error.NlError,
            };
        }

        msg_pos += nlmsg_align(msg_hdr.len);
    }

    std.time.sleep(500 * std.time.ns_per_ms);
}

fn getScanResults(
    sock: *NlSocket,
    family_id: u16,
    if_index: i32,
    out_networks: []CWifiNetwork,
) !u32 {
    var buf: [4096]u8 align(4) = undefined;
    var builder = NlMsgBuilder.init(&buf);

    const seq = sock.nextSeq();
    const hdr = try builder.putNlmsghdr(family_id, NLM_F_REQUEST | NLM_F_DUMP, seq);
    try builder.putGenlmsghdr(NL80211_CMD_GET_SCAN, 0);
    try builder.putAttrU32(NL80211_ATTR_IFINDEX, @bitCast(if_index));
    builder.finalize(hdr);

    try sock.send(buf[0..hdr.len]);

    var count: u32 = 0;
    debugLog("Requesting scan dump from kernel...\n", .{});
    var recv_buf: [32768]u8 align(4) = undefined;

    outer: while (true) {
        const n = try sock.recv(&recv_buf);
        if (n == 0) break;

        var msg_pos: usize = 0;

        while (msg_pos + @sizeOf(NlMsgHeader) <= n) {
            const msg_hdr: *const NlMsgHeader = @ptrCast(@alignCast(recv_buf[msg_pos..].ptr));

            if (msg_hdr.len < @sizeOf(NlMsgHeader) or msg_hdr.len > n - msg_pos) break;

            if (msg_hdr.type_ == NLMSG_DONE) {
                debugLog("Hit NLMSG_DONE\n", .{});
                break :outer;
            }
            if (msg_hdr.type_ == NLMSG_ERROR) {
                const err_code_ptr: *const i32 = @ptrCast(@alignCast(recv_buf[msg_pos + @sizeOf(NlMsgHeader) ..][0..4]));
                if (err_code_ptr.* == 0) {
                    debugLog("Hit ACK, continuing...\n", .{});
                    msg_pos += nlmsg_align(msg_hdr.len);
                    continue;
                }

                debugLog("Hit NLMSG_ERROR with code {}\n", .{err_code_ptr.*});
                return switch (err_code_ptr.*) {
                    -1 => error.PermissionDenied,
                    else => error.NlError,
                };
            }

            debugLog("Got message len={} type={}\n", .{ msg_hdr.len, msg_hdr.type_ });

            if (count < out_networks.len) {
                if (parseBssMessage(
                    recv_buf[msg_pos .. msg_pos + msg_hdr.len],
                    &out_networks[count],
                )) {
                    count += 1;
                } else |_| {}
            }

            msg_pos += nlmsg_align(msg_hdr.len);
        }
    }

    debugLog("Total networks parsed: {}\n", .{count});
    return count;
}

// ──────────────────────────────────────────────
// SECTION 8: Parse BSS (one network from scan)
// ──────────────────────────────────────────────

fn parseBssMessage(msg: []const u8, network: *CWifiNetwork) !void {
    network.* = std.mem.zeroes(CWifiNetwork);

    const attr_start = @sizeOf(NlMsgHeader) + @sizeOf(GenlMsgHeader);
    if (msg.len < attr_start) return error.MessageTooShort;

    var pos: usize = attr_start;
    while (pos + @sizeOf(NlAttrHeader) <= msg.len) {
        const attr: *const NlAttrHeader = @ptrCast(@alignCast(msg[pos..].ptr));
        if (attr.len < @sizeOf(NlAttrHeader)) break;

        if (attr.type_ == NL80211_ATTR_BSS) {
            debugLog("Got NL80211_ATTR_BSS attribute!\n", .{});
            const nested_start = pos + @sizeOf(NlAttrHeader);
            const nested_end = pos + attr.len;
            try parseBssAttrs(msg[nested_start..nested_end], network);
            return;
        }

        pos += nlmsg_align(attr.len);
    }

    return error.NoBssAttr;
}

fn parseBssAttrs(data: []const u8, network: *CWifiNetwork) !void {
    var pos: usize = 0;

    while (pos + @sizeOf(NlAttrHeader) <= data.len) {
        const attr: *const NlAttrHeader = @ptrCast(@alignCast(data[pos..].ptr));
        if (attr.len < @sizeOf(NlAttrHeader)) break;

        const payload_start = pos + @sizeOf(NlAttrHeader);
        const payload_len = attr.len - @sizeOf(NlAttrHeader);

        switch (attr.type_) {
            NL80211_BSS_BSSID => {
                if (payload_len >= 6) {
                    @memcpy(&network.bssid, data[payload_start..][0..6]);
                }
            },
            NL80211_BSS_FREQUENCY => {
                if (payload_len >= 4) {
                    const freq: *const u32 = @ptrCast(@alignCast(data[payload_start..].ptr));
                    network.frequency = freq.*;
                    network.channel = freqToChannel(freq.*);
                }
            },
            NL80211_BSS_SIGNAL_MBM => {
                if (payload_len >= 4) {
                    const mbm: *const i32 = @ptrCast(@alignCast(data[payload_start..].ptr));
                    network.signal_dbm = @divTrunc(mbm.*, 100);
                }
            },
            NL80211_BSS_INFORMATION_ELEMENTS => {
                parseInformationElements(
                    data[payload_start .. payload_start + payload_len],
                    network,
                );
            },
            NL80211_BSS_CAPABILITY => {
                if (payload_len >= 2) {
                    const cap: *const u16 = @ptrCast(@alignCast(data[payload_start..].ptr));
                    if (cap.* & 0x0010 != 0 and network.security == 0) {
                        network.security = @intFromEnum(SecurityType.WEP);
                    }
                }
            },
            NL80211_BSS_STATUS => {
                if (payload_len >= 4) {
                    const status: *const u32 = @ptrCast(@alignCast(data[payload_start..].ptr));
                    network.bss_status = switch (status.*) {
                        NL80211_BSS_STATUS_AUTHENTICATED => 1,
                        NL80211_BSS_STATUS_ASSOCIATED => 2,
                        NL80211_BSS_STATUS_IBSS_JOINED => 3,
                        else => 0,
                    };
                }
            },
            NL80211_BSS_BEACON_INTERVAL => {
                if (payload_len >= 2) {
                    const bi: *const u16 = @ptrCast(@alignCast(data[payload_start..].ptr));
                    network.beacon_interval = bi.*;
                }
            },
            else => {},
        }

        pos += nlmsg_align(attr.len);
    }
}

// ──────────────────────────────────────────────
// SECTION 9: Parse Information Elements
// ──────────────────────────────────────────────

fn parseInformationElements(ie_data: []const u8, network: *CWifiNetwork) void {
    var pos: usize = 0;

    while (pos + 2 <= ie_data.len) {
        const tag = ie_data[pos];
        const len = ie_data[pos + 1];
        pos += 2;

        if (pos + len > ie_data.len) break;

        switch (tag) {
            0 => { // SSID
                const ssid_len = @min(len, 32);
                @memcpy(network.ssid[0..ssid_len], ie_data[pos..][0..ssid_len]);
                network.ssid[ssid_len] = 0;
            },
            48 => { // RSN Information (WPA2 or WPA3)
                if (detectWpa3(ie_data[pos .. pos + len])) {
                    network.security = @intFromEnum(SecurityType.WPA3);
                } else {
                    network.security = @intFromEnum(SecurityType.WPA2);
                }
            },
            221 => { // Vendor Specific
                if (len >= 4 and
                    ie_data[pos] == 0x00 and
                    ie_data[pos + 1] == 0x50 and
                    ie_data[pos + 2] == 0xF2 and
                    ie_data[pos + 3] == 0x01)
                {
                    if (network.security < @intFromEnum(SecurityType.WPA2)) {
                        network.security = @intFromEnum(SecurityType.WPA);
                    }
                }
            },
            else => {},
        }

        pos += len;
    }
}

fn detectWpa3(rsn_data: []const u8) bool {
    if (rsn_data.len < 10) return false;
    var pos: usize = 2 + 4;
    if (pos + 2 > rsn_data.len) return false;
    const pw_count = std.mem.readInt(u16, rsn_data[pos..][0..2], .little);
    pos += 2 + pw_count * 4;
    if (pos + 2 > rsn_data.len) return false;
    const akm_count = std.mem.readInt(u16, rsn_data[pos..][0..2], .little);
    pos += 2;
    var i: u16 = 0;
    while (i < akm_count) : (i += 1) {
        if (pos + 4 > rsn_data.len) return false;
        if (rsn_data[pos + 3] == 8) return true; // SAE
        pos += 4;
    }
    return false;
}

// ──────────────────────────────────────────────
// SECTION 10: Frequency → Channel Conversion
// ──────────────────────────────────────────────

pub fn freqToChannel(freq: u32) u8 {
    // 2.4 GHz band
    if (freq >= 2412 and freq <= 2484) {
        if (freq == 2484) return 14;
        return @intCast((freq - 2407) / 5);
    }
    // 5 GHz band
    if (freq >= 5170 and freq <= 5835) {
        return @intCast((freq - 5000) / 5);
    }
    // 6 GHz band (Wi-Fi 6E)
    if (freq >= 5955 and freq <= 7115) {
        return @intCast((freq - 5950) / 5);
    }
    return 0;
}

// ──────────────────────────────────────────────
// SECTION 11: EXPORTED C ABI FUNCTIONS
// ──────────────────────────────────────────────

var g_networks: [MAX_NETWORKS]CWifiNetwork = undefined;
var g_result: CScanResult = undefined;

/// Perform a WiFi scan and return results.
export fn wifi_scan(iface_name: [*:0]const u8) *CScanResult {
    const name = std.mem.span(iface_name);

    g_result = .{
        .networks = &g_networks,
        .count = 0,
        .error_code = 0,
    };

    doScan(name) catch |err| {
        g_result.error_code = switch (@as(anyerror, err)) {
            error.PermissionDenied => -1,
            error.IoctlFailed => -2,
            error.FamilyNotFound => -3,
            error.NlError => -4,
            else => -99,
        };
    };

    return &g_result;
}

/// Get cached scan results WITHOUT triggering a new scan.
export fn wifi_get_cached(iface_name: [*:0]const u8) *CScanResult {
    const name = std.mem.span(iface_name);

    g_result = .{
        .networks = &g_networks,
        .count = 0,
        .error_code = 0,
    };

    getCached(name) catch |err| {
        g_result.error_code = switch (@as(anyerror, err)) {
            error.PermissionDenied => -1,
            error.IoctlFailed => -2,
            error.FamilyNotFound => -3,
            else => -99,
        };
    };

    return &g_result;
}

/// Returns the library version
export fn wifi_scanner_version() u32 {
    return 2_000; // v2.0.0
}

/// Returns capability bitmask
export fn wifi_scanner_capabilities() u32 {
    return CAP_SCAN_TRIGGER |
        CAP_CACHED_RESULTS |
        CAP_BSS_STATUS |
        CAP_BEACON_INTERVAL |
        CAP_AUTO_IFACE |
        CAP_SECURITY_DETECT;
}

/// Set verbose mode (1 = verbose, 0 = quiet)
export fn wifi_set_verbose(verbose: u32) void {
    g_verbose = verbose != 0;
}

/// Auto-detect WiFi interface name. Returns pointer to null-terminated string.
/// Returns null if no WiFi interface found.
var g_auto_iface: [16]u8 = undefined;
export fn wifi_auto_detect_interface() ?[*:0]const u8 {
    if (autoDetectWifiInterface(&g_auto_iface)) {
        // Find null terminator position to return sentinel
        for (0..16) |i| {
            if (g_auto_iface[i] == 0) {
                return @ptrCast(g_auto_iface[0..i :0]);
            }
        }
    }
    return null;
}

// ──────────────────────────────────────────────
// SECTION 12: Internal Scan Orchestration
// ──────────────────────────────────────────────

fn doScan(iface: []const u8) !void {
    var sock = try NlSocket.open();
    defer sock.close();

    const family_id = try resolveNl80211Family(&sock);
    const if_index = try getIfIndex(iface);

    triggerScan(&sock, family_id, if_index) catch {};

    g_result.count = try getScanResults(&sock, family_id, if_index, &g_networks);
}

fn getCached(iface: []const u8) !void {
    var sock = try NlSocket.open();
    defer sock.close();

    const family_id = try resolveNl80211Family(&sock);
    const if_index = try getIfIndex(iface);

    g_result.count = try getScanResults(&sock, family_id, if_index, &g_networks);
}

// ──────────────────────────────────────────────
// SECTION 13: Unit Tests
// ──────────────────────────────────────────────

test "freqToChannel: 2.4 GHz band" {
    try std.testing.expectEqual(@as(u8, 1), freqToChannel(2412));
    try std.testing.expectEqual(@as(u8, 6), freqToChannel(2437));
    try std.testing.expectEqual(@as(u8, 11), freqToChannel(2462));
    try std.testing.expectEqual(@as(u8, 14), freqToChannel(2484));
}

test "freqToChannel: 5 GHz band" {
    try std.testing.expectEqual(@as(u8, 36), freqToChannel(5180));
    try std.testing.expectEqual(@as(u8, 40), freqToChannel(5200));
    try std.testing.expectEqual(@as(u8, 149), freqToChannel(5745));
}

test "freqToChannel: 6 GHz band" {
    try std.testing.expectEqual(@as(u8, 1), freqToChannel(5955));
    try std.testing.expectEqual(@as(u8, 5), freqToChannel(5975));
    try std.testing.expectEqual(@as(u8, 9), freqToChannel(5995));
}

test "freqToChannel: unknown returns 0" {
    try std.testing.expectEqual(@as(u8, 0), freqToChannel(123));
    try std.testing.expectEqual(@as(u8, 0), freqToChannel(0));
    try std.testing.expectEqual(@as(u8, 0), freqToChannel(9999));
}

test "nlmsg_align: rounds up to 4-byte boundary" {
    try std.testing.expectEqual(@as(u32, 4), nlmsg_align(1));
    try std.testing.expectEqual(@as(u32, 4), nlmsg_align(2));
    try std.testing.expectEqual(@as(u32, 4), nlmsg_align(3));
    try std.testing.expectEqual(@as(u32, 4), nlmsg_align(4));
    try std.testing.expectEqual(@as(u32, 8), nlmsg_align(5));
    try std.testing.expectEqual(@as(u32, 16), nlmsg_align(16));
}

test "detectWpa3: returns false on empty data" {
    try std.testing.expect(!detectWpa3(&[_]u8{}));
    try std.testing.expect(!detectWpa3(&[_]u8{ 0, 0, 0, 0, 0 }));
}

test "NlMsgBuilder: constructs valid message" {
    var buf: [256]u8 align(4) = undefined;
    var builder = NlMsgBuilder.init(&buf);

    const hdr = try builder.putNlmsghdr(42, NLM_F_REQUEST, 1);
    try builder.putGenlmsghdr(NL80211_CMD_GET_SCAN, 0);
    try builder.putAttrU32(NL80211_ATTR_IFINDEX, 3);
    builder.finalize(hdr);

    try std.testing.expect(hdr.len > 0);
    try std.testing.expectEqual(@as(u16, 42), hdr.type_);
}

test "CWifiNetwork: zeroed initialization" {
    const nw = std.mem.zeroes(CWifiNetwork);
    try std.testing.expectEqual(@as(i32, 0), nw.signal_dbm);
    try std.testing.expectEqual(@as(u32, 0), nw.frequency);
    try std.testing.expectEqual(@as(u8, 0), nw.channel);
    try std.testing.expectEqual(@as(u8, 0), nw.security);
    try std.testing.expectEqual(@as(u8, 0), nw.bss_status);
    try std.testing.expectEqual(@as(u16, 0), nw.beacon_interval);
}
