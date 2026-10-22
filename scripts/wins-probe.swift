// Window/activation probe for acceptance evidence (no Screen Recording needed).
//
//   swift scripts/wins-probe.swift <pid>
//
// Prints NSRunningApplication.activationPolicy (0=regular, 1=accessory,
// 2=prohibited) and every on-screen window owned by Agent Monitor with its
// Cocoa window bounds. Used to verify: accessory policy (no Dock icon), pet
// geometry per max_visible_rows, and that the settings window is a separate
// native window. Window *titles* additionally require Screen Recording
// permission; bounds/owner do not.
import AppKit

let args = CommandLine.arguments
guard args.count >= 2, let pid = Int32(args[1]) else {
    FileHandle.standardError.write(Data("usage: swift scripts/wins-probe.swift <pid>\n".utf8))
    exit(2)
}

if let app = NSRunningApplication(processIdentifier: pid) {
    let policy: String
    switch app.activationPolicy {
    case .regular: policy = "regular"
    case .accessory: policy = "accessory"
    case .prohibited: policy = "prohibited"
    @unknown default: policy = "unknown"
    }
    print("activationPolicy=\(policy) backgroundOnly=\(app.activationPolicy != .regular) name=\(app.localizedName ?? "?") bundle=\(app.bundleIdentifier ?? "-")")
} else {
    print("activationPolicy=no-running-application")
}

let windows = CGWindowListCopyWindowInfo(
    [.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID
) as? [[String: Any]] ?? []

var found = 0
for window in windows {
    let owner = window[kCGWindowOwnerName as String] as? String ?? ""
    guard owner.contains("agent-monitor") || owner.contains("Agent Monitor") else { continue }
    found += 1
    let layer = window[kCGWindowLayer as String] as? Int ?? -1
    let bounds = window[kCGWindowBounds as String] as? [String: Any] ?? [:]
    let x = bounds["X"] ?? "?"
    let y = bounds["Y"] ?? "?"
    let w = bounds["Width"] ?? "?"
    let h = bounds["Height"] ?? "?"
    print("window#\(found) owner=\(owner) layer=\(layer) x=\(x) y=\(y) w=\(w) h=\(h)")
}
if found == 0 {
    print("windows=none-visible")
}
