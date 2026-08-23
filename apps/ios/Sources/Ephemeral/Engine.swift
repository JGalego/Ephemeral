// The only thing in this application allowed to talk to the engine.
//
// Everything below the C ABI is the same Rust the desktop runs: the lifecycle
// machine, the permission ledger, the audit record. This file is a wrapper, not
// a second implementation — a screen that decided for itself whether a
// permission was dangerous would be a second, subtly different Ephemeral, and
// the two would disagree within a month.
//
// The views never see a handle. They see values, and the values come from the
// engine's own JSON.

import Ephemeral
import Foundation

/// Something the engine refused to do, in the words it used.
struct EngineFailure: LocalizedError {
    let what: String
    var errorDescription: String? { what }
}

/// One application, as it appears in a list.
struct Summary: Identifiable, Hashable {
    let id: String
    let name: String
    let purpose: String
    let state: String
    /// The same state as the kind of thing it is, which is what decides its
    /// colour. Carried from the engine so a phone cannot hold its own opinion
    /// about which states are alarming.
    let stateKind: String
    let granted: Int
    /// The worst thing it is allowed to do, if it is allowed anything.
    let highestGrantedRisk: String?
    let awaitingDecision: Int
    let putAway: Bool

    init(_ json: [String: Any]) {
        id = json["id"] as? String ?? ""
        name = json["name"] as? String ?? ""
        purpose = json["purpose"] as? String ?? ""
        state = json["state"] as? String ?? ""
        stateKind = json["state_kind"] as? String ?? ""
        granted = json["granted"] as? Int ?? 0
        // Absent and empty are different: an unknown risk is drawn as unknown
        // rather than defaulted to something reassuring.
        highestGrantedRisk = (json["highest_granted_risk"] as? String)?.nilIfEmpty
        awaitingDecision = json["awaiting_decision"] as? Int ?? 0
        putAway = json["put_away"] as? Bool ?? false
    }
}

/// One capability, in the words the engine chose for it.
struct Capability: Identifiable, Hashable {
    var id: String { capability }

    let capability: String
    let wants: String
    /// Written by whatever asked for the permission, and the one part of a
    /// request a person cannot check. Presented as a claim, never as a fact.
    let reason: String?
    let ifAllowed: String
    let risk: String
    let needsExplicitConfirmation: Bool

    init(_ json: [String: Any]) {
        capability = json["capability"] as? String ?? ""
        wants = json["wants"] as? String ?? ""
        reason = (json["reason"] as? String)?.nilIfEmpty
        ifAllowed = json["if_allowed"] as? String ?? ""
        risk = json["risk"] as? String ?? ""
        needsExplicitConfirmation = json["needs_explicit_confirmation"] as? Bool ?? false
    }
}

/// One application's whole page.
struct Detail {
    let summary: Summary
    let explanation: String
    let description: String
    let outstanding: [Capability]
    let allowed: [Capability]
    let isolated: Bool
    let versions: Int
    let retention: String

    init(_ json: [String: Any]) {
        summary = Summary(json["summary"] as? [String: Any] ?? [:])
        explanation = json["explanation"] as? String ?? ""
        description = json["description"] as? String ?? ""

        let permissions = json["permissions"] as? [String: Any] ?? [:]
        outstanding = Engine.capabilities(permissions["outstanding"])
        allowed = Engine.capabilities(permissions["allowed"])
        isolated = permissions["isolated"] as? Bool ?? false

        versions = (json["versions"] as? [Any])?.count ?? 0
        retention = json["retention"] as? String ?? ""
    }
}

/// Ephemeral, on this phone.
///
/// One instance for the life of the application. Opening it twice would be two
/// processes writing one workspace, which the storage layer is not built for
/// and which no screen needs.
final class Engine {
    static let shared = Engine()

    private var handle: OpaquePointer?
    /// Every call into the engine happens here. The library is not thread-safe
    /// by contract, and generating takes minutes, so the rule is: one queue,
    /// and never the main one.
    private let queue = DispatchQueue(label: "sh.ephemeral.engine")

    private init() {}

    // MARK: - Opening

    /// Opens the engine, if it is not already open.
    ///
    /// Files go in Application Support rather than Documents: an application's
    /// own storage is not the user's documents, and this directory is excluded
    /// from iCloud backup so a model key's neighbours never leave the device.
    func open() throws {
        if handle != nil { return }

        let home = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        ).appendingPathComponent("Ephemeral", isDirectory: true)

        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        var excluded = URLResourceValues()
        excluded.isExcludedFromBackup = true
        var backupless = home
        try? backupless.setResourceValues(excluded)

        guard let opened = ephemeral_open(home.path, Transport.send, Transport.release, nil) else {
            throw EngineFailure(what: "Ephemeral could not open its files on this device.")
        }

        handle = opened

        // Both halves of "which model", restored together. A session that came
        // back with the credential but not the choice would quietly send it to
        // whichever service is the default — which is somebody else's.
        if let chosen = Choice.stored {
            _ = ephemeral_set_provider(handle, chosen.asJson)
        }
    }

    /// Hands the engine the model credential, from the Keychain.
    ///
    /// Never from an environment variable: that is a desktop convention, and
    /// the library deliberately does not look for one.
    func useCredential(_ key: String) throws {
        try open()
        guard ephemeral_set_credential(handle, key) == 0 else {
            throw failure("The model key was refused.")
        }
    }

    /// Chooses which service generates, and how it is configured.
    ///
    /// Saved on this side too, because the engine holds the choice for the life
    /// of a session and a phone's sessions end whenever the system decides they
    /// do. The credential is not part of it: that is in the Keychain, and
    /// keeping the two apart is what lets this live in ordinary preferences.
    func choose(_ choice: Choice) throws {
        try open()
        guard ephemeral_set_provider(handle, choice.asJson) == 0 else {
            throw failure("That provider was refused.")
        }
        choice.save()
    }

    /// Every provider this build can be pointed at.
    ///
    /// Read from the engine rather than listed here. A list of providers in the
    /// application is a list that is wrong the moment one is added, and the
    /// application and the engine do not ship on the same schedule.
    ///
    /// The one call that needs no handle and no queue: it answers from a fixed
    /// list and touches nothing.
    static func providers() -> [Provider] {
        guard let produced = ephemeral_providers() else { return [] }
        defer { ephemeral_string_free(produced) }

        let json = try? JSONSerialization.jsonObject(with: Data(String(cString: produced).utf8))
        return (json as? [[String: Any]] ?? []).map(Provider.init)
    }

    // MARK: - What the screens call

    func create(intent: String) throws -> Summary {
        Summary(try object(ephemeral_create(handle, intent)))
    }

    func applications() throws -> [Summary] {
        try array(ephemeral_applications(handle)).map(Summary.init)
    }

    func application(_ id: String) throws -> Detail {
        Detail(try object(ephemeral_application(handle, id)))
    }

    /// Plans and writes an application, and stops there.
    ///
    /// Deliberately does not build, run or test it: that needs a sandbox no
    /// phone has, and running generated code outside one is the thing Ephemeral
    /// exists to prevent. Minutes long, so nothing calls this on the main
    /// thread.
    func generate(_ id: String) throws -> Detail {
        Detail(try object(ephemeral_generate(handle, id)))
    }

    func decide(_ id: String, capability: String, allow: Bool) throws {
        guard ephemeral_decide(handle, id, capability, allow) == 0 else {
            throw failure("That decision could not be recorded.")
        }
    }

    /// Runs some work against the engine off the main thread and comes back on
    /// it, because every screen that calls this is about to redraw.
    func submit<Value>(
        _ work: @escaping () throws -> Value,
        then done: @escaping (Result<Value, Error>) -> Void
    ) {
        queue.async {
            let outcome = Result { try self.open(); return try work() }
            DispatchQueue.main.async { done(outcome) }
        }
    }

    // MARK: - Turning C into Swift

    /// Reads a string the library allocated, and frees it exactly once.
    private func take(_ pointer: UnsafeMutablePointer<CChar>?) throws -> Data {
        guard let pointer else { throw failure("Ephemeral did not answer.") }
        defer { ephemeral_string_free(pointer) }
        return Data(String(cString: pointer).utf8)
    }

    private func object(_ pointer: UnsafeMutablePointer<CChar>?) throws -> [String: Any] {
        let json = try JSONSerialization.jsonObject(with: try take(pointer))
        guard let object = json as? [String: Any] else {
            throw failure("Ephemeral answered with something unreadable.")
        }
        return object
    }

    private func array(_ pointer: UnsafeMutablePointer<CChar>?) throws -> [[String: Any]] {
        let json = try JSONSerialization.jsonObject(with: try take(pointer))
        return json as? [[String: Any]] ?? []
    }

    static func capabilities(_ raw: Any?) -> [Capability] {
        (raw as? [[String: Any]] ?? []).map(Capability.init)
    }

    /// The engine's own account of what went wrong, if it has one.
    ///
    /// Preferred over anything this file could invent: the engine's wording is
    /// what the terminal and the window show, and three clients paraphrasing
    /// one failure differently is three bug reports about three bugs.
    private func failure(_ fallback: String) -> EngineFailure {
        guard let said = ephemeral_last_error(handle) else {
            return EngineFailure(what: fallback)
        }
        defer { ephemeral_string_free(said) }

        let text = String(cString: said)
        return EngineFailure(what: text.isEmpty ? fallback : text)
    }
}

extension String {
    /// Absent and empty are different everywhere in this application.
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
