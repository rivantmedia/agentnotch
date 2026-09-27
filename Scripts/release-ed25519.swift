// Ed25519 for Sparkle's update signatures, with CryptoKit: nothing to install,
// and never the Keychain (Sparkle's own generate_keys keeps its key there,
// which a CI runner cannot use and a maintainer's login keychain should not
// hold as the only copy).
//
//   swift Scripts/release-ed25519.swift generate <seed-file>
//       a new key pair: the private seed goes into <seed-file> (created, 0600,
//       never overwritten); the public key is printed
//   swift Scripts/release-ed25519.swift public < <seed-file>
//       the public key of the private seed read from stdin
//   swift Scripts/release-ed25519.swift verify <public-key> <file> <signature>
//       exit 0 when <signature> is a valid signature of <file>'s bytes
//
// Sparkle's formats: the private key is the base64 of the 32-byte seed (what
// `sign_update --ed-key-file` reads and the SPARKLE_ED_PRIVATE_KEY secret
// holds), the public key the base64 of the 32-byte raw key (SUPublicEDKey),
// and a signature the base64 of 64 bytes over the whole file.
//
// The seed only ever travels through a file or stdin: never argv, where `ps`
// shows it, and never stdout.
import CryptoKit
import Foundation

func fail(_ message: String, code: Int32 = 2) -> Never {
    FileHandle.standardError.write(Data("release-ed25519: \(message)\n".utf8))
    exit(code)
}

func decode(_ text: String, bytes: Int, what: String) -> Data {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard let data = Data(base64Encoded: trimmed), data.count == bytes else {
        fail("\(what) is not the base64 of \(bytes) bytes")
    }
    return data
}

let args = Array(CommandLine.arguments.dropFirst())
switch args.first {
case "generate" where args.count == 2:
    let key = Curve25519.Signing.PrivateKey()
    let seed = Data((key.rawRepresentation.base64EncodedString() + "\n").utf8)
    // O_EXCL: an existing private key is never replaced, whatever it holds.
    let fd = open(args[1], O_WRONLY | O_CREAT | O_EXCL, 0o600)
    guard fd >= 0 else { fail("cannot create \(args[1]): \(String(cString: strerror(errno)))") }
    let written = seed.withUnsafeBytes { write(fd, $0.baseAddress, $0.count) }
    guard written == seed.count, fsync(fd) == 0, close(fd) == 0 else {
        fail("cannot write \(args[1])")
    }
    print(key.publicKey.rawRepresentation.base64EncodedString())

case "public" where args.count == 1:
    let input = String(decoding: FileHandle.standardInput.readDataToEndOfFile(), as: UTF8.self)
    let seed = decode(input, bytes: 32, what: "the private key on stdin")
    guard let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: seed) else {
        fail("the private key on stdin is not an Ed25519 seed")
    }
    print(key.publicKey.rawRepresentation.base64EncodedString())

case "verify" where args.count == 4:
    let raw = decode(args[1], bytes: 32, what: "the public key")
    let signature = decode(args[3], bytes: 64, what: "the signature")
    guard let key = try? Curve25519.Signing.PublicKey(rawRepresentation: raw) else {
        fail("the public key is not an Ed25519 key")
    }
    guard let file = try? Data(contentsOf: URL(fileURLWithPath: args[2]), options: .alwaysMapped) else {
        fail("cannot read \(args[2])")
    }
    guard key.isValidSignature(signature, for: file) else {
        fail("the signature of \(args[2]) does not verify with that public key", code: 1)
    }
    print("signature verified: \(args[2])")

default:
    fail("usage: generate <seed-file> | public < <seed-file> | verify <public-key> <file> <signature>")
}
