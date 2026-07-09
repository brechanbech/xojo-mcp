#!/bin/sh
# Send an IDE script to the running Xojo IDE and print the response.
# Usage: scripts/ide-test.sh 'Print "hello"'
set -e

SOCKET="/tmp/XojoIDE"
TAG="test_$$"
SCRIPT="$1"

if [ -z "$SCRIPT" ]; then
    echo "Usage: $0 '<ide script>'" >&2
    exit 1
fi

if [ ! -S "$SOCKET" ]; then
    echo "Error: Xojo IDE socket not found at $SOCKET" >&2
    exit 1
fi

# Use swift for reliable Unix socket I/O.
swift - "$SOCKET" "$TAG" "$SCRIPT" <<'SWIFT'
import Foundation

let socket_path = CommandLine.arguments[1]
let tag = CommandLine.arguments[2]
let script = CommandLine.arguments[3]

let fd = socket(AF_UNIX, SOCK_STREAM, 0)
guard fd >= 0 else { fputs("socket() failed\n", stderr); exit(1) }

var addr = sockaddr_un()
addr.sun_family = sa_family_t(AF_UNIX)
withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
    socket_path.withCString { src in
        _ = memcpy(ptr, src, min(Int(MemoryLayout.size(ofValue: addr.sun_path)), socket_path.utf8.count + 1))
    }
}

let result = withUnsafePointer(to: &addr) { ptr in
    ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
}
guard result == 0 else { fputs("connect() failed: \(errno)\n", stderr); exit(1) }

// Build payload: two NUL-delimited JSON frames.
let proto = #"{"protocol":2}"#
let escapedScript = String(data: try! JSONSerialization.data(withJSONObject: script, options: [.fragmentsAllowed]), encoding: .utf8)!
let request = #"{"tag":"\#(tag)","script":\#(escapedScript)}"#
var payload = Data()
payload.append(proto.data(using: .utf8)!)
payload.append(0)
payload.append(request.data(using: .utf8)!)
payload.append(0)

_ = payload.withUnsafeBytes { send(fd, $0.baseAddress!, $0.count, 0) }

// Read response with 5-second timeout.
var tv = timeval(tv_sec: 5, tv_usec: 0)
setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

var buf = [UInt8](repeating: 0, count: 65536)
let n = recv(fd, &buf, buf.count, 0)
close(fd)

if n > 0 {
    // Split on NUL, print each frame.
    let data = Data(buf[0..<n])
    for chunk in data.split(separator: 0) {
        if let s = String(data: chunk, encoding: .utf8) { print(s) }
    }
} else {
    fputs("No response received\n", stderr)
    exit(1)
}
SWIFT
