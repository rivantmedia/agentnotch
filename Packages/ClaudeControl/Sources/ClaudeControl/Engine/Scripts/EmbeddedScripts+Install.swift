//
//  EmbeddedScripts+Install.swift
//  ClaudeControl
//
//  The scripts as they are written into `<configDir>/hooks/`: the embedded
//  sources with the app's socket path filled in.
//

import Foundation

nonisolated enum EmbeddedScripts {
    /// The one token in each script replaced at install time, quotes included,
    /// so only the Python string literal is ever touched.
    static let socketPlaceholder = "__AGENTNOTCH_SOCKET_PATH__"
    static let quotedPlaceholder = "\"" + socketPlaceholder + "\""

    /// The hook script for `socketPath`.
    static func hook(socketPath: String) -> String {
        fill(EmbeddedScriptSources.hook, socketPath: socketPath)
    }

    /// The status line wrapper for `socketPath`.
    static func statusLine(socketPath: String) -> String {
        fill(EmbeddedScriptSources.statusLine, socketPath: socketPath)
    }

    /// Replace the quoted placeholder with `socketPath` as a Python string literal.
    static func fill(_ template: String, socketPath: String) -> String {
        template.replacingOccurrences(of: quotedPlaceholder, with: pythonStringLiteral(socketPath))
    }

    /// A double-quoted Python literal for `value`: backslashes, quotes and
    /// control characters escaped, everything else as is (Python 3 source is UTF-8).
    static func pythonStringLiteral(_ value: String) -> String {
        var out = "\""
        for scalar in value.unicodeScalars {
            switch scalar {
            case "\\": out += "\\\\"
            case "\"": out += "\\\""
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            default:
                if scalar.value < 0x20 || scalar.value == 0x7F {
                    out += String(format: "\\x%02x", scalar.value)
                } else {
                    out.unicodeScalars.append(scalar)
                }
            }
        }
        return out + "\""
    }
}
