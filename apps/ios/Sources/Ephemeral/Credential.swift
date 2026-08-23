// The model key, in the Keychain.
//
// It is a credential belonging to the person, not to Ephemeral, so it lives
// where the platform keeps credentials and nowhere else. It is never written to
// Ephemeral's own files, never printed, and never put in a log — the engine's
// audit record redacts on write for the same reason, and this is the half of
// that promise which lives outside the engine.
//
// `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`: a key that syncs to iCloud or
// survives a restore onto another device is a key on a device its owner did not
// think they were putting it on.

import Foundation
import Security

enum Credential {
    private static let service = "sh.ephemeral.model-key"
    private static let account = "anthropic"

    /// Whether there is a key at all, without reading it.
    ///
    /// Screens ask this to decide whether to offer generating. Reading the key
    /// to answer a question about its existence would be handling a secret for
    /// no reason.
    static var present: Bool {
        var query = base
        query[kSecReturnData as String] = false
        return SecItemCopyMatching(query as CFDictionary, nil) == errSecSuccess
    }

    /// Reads it, for the one call that needs it.
    static func read() -> String? {
        var query = base
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var found: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &found) == errSecSuccess,
              let data = found as? Data
        else { return nil }

        return String(data: data, encoding: .utf8)
    }

    /// Saves it, replacing whatever was there.
    @discardableResult
    static func save(_ key: String) -> Bool {
        forget()

        var item = base
        item[kSecValueData as String] = Data(key.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly

        return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
    }

    /// Removes it. Offered on the same screen that sets it, because a key you
    /// cannot take back is a key you did not really lend.
    @discardableResult
    static func forget() -> Bool {
        SecItemDelete(base as CFDictionary) == errSecSuccess
    }

    private static var base: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }
}
