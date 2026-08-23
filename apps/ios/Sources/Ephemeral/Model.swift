// Which service generates, and how it is configured.
//
// The engine used to build an Anthropic provider and nothing else, so there was
// nothing here to choose and no file like this. That was the platform deciding
// the vendor, which is not a decision a platform should make: generating is one
// HTTPS request, and which company answers it belongs to the person paying for
// it.
//
// The credential is deliberately not part of any of this. It is in the Keychain
// (`Credential.swift`); a choice is ordinary preferences, and keeping the two
// apart is what makes that separation real rather than stated.

import Foundation

/// One service this build can be pointed at, as the engine describes it.
struct Provider: Identifiable, Hashable {
    var id: String { name }

    let name: String
    /// One line, for somebody choosing.
    let what: String
    let needsCredential: Bool
    /// Which of `Choice`'s fields this provider reads. Show these, hide the
    /// rest: a model box on the mock is a box that does nothing, and somebody
    /// who fills one in and sees no difference has learnt the screen lies.
    let configurable: [String]
    let baseUrl: String?
    let model: String?

    init(_ json: [String: Any]) {
        name = json["name"] as? String ?? ""
        what = json["what"] as? String ?? ""
        needsCredential = json["needs_credential"] as? Bool ?? false
        configurable = json["configurable"] as? [String] ?? []
        baseUrl = (json["base_url"] as? String)?.nilIfEmpty
        model = (json["model"] as? String)?.nilIfEmpty
    }

    func configures(_ field: String) -> Bool { configurable.contains(field) }
}

/// What somebody chose.
struct Choice: Equatable {
    var provider: String
    var baseUrl: String = ""
    var model: String = ""

    /// The JSON the engine takes. An absent field means that provider's own
    /// default, so a blank box is left out rather than sent as an empty string.
    var asJson: String {
        var body: [String: String] = ["provider": provider]
        if !baseUrl.trimmed.isEmpty { body["base_url"] = baseUrl.trimmed }
        if !model.trimmed.isEmpty { body["model"] = model.trimmed }

        let data = (try? JSONSerialization.data(withJSONObject: body)) ?? Data()
        return String(data: data, encoding: .utf8) ?? #"{"provider":"anthropic"}"#
    }

    // MARK: - Remembering it

    private static let key = "sh.ephemeral.model"

    /// What was chosen last, or nil if nobody has chosen yet.
    static var stored: Choice? {
        guard let saved = UserDefaults.standard.dictionary(forKey: key),
              let provider = saved["provider"] as? String
        else { return nil }

        return Choice(
            provider: provider,
            baseUrl: saved["base_url"] as? String ?? "",
            model: saved["model"] as? String ?? ""
        )
    }

    func save() {
        UserDefaults.standard.set(
            ["provider": provider, "base_url": baseUrl.trimmed, "model": model.trimmed],
            forKey: Self.key
        )
    }
}

extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
