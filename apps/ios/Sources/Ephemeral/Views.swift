// What the application shows.
//
// The same three facts the window shows about an application, in the same
// words and the same colours: what it is, where it is in its life, and what it
// is allowed to do. Every phrase a person reads here comes from the engine —
// nothing in this file writes a sentence about a permission, because a phone
// that phrased one differently from the terminal would be making a second
// promise.

import SwiftUI

// MARK: - The list

struct ApplicationList: View {
    @State private var applications: [Summary] = []
    @State private var intent = ""
    @State private var problem: String?
    @State private var creating = false
    @State private var showingKey = false

    var body: some View {
        NavigationStack {
            ZStack {
                Palette.ground.ignoresSafeArea()

                VStack(spacing: 0) {
                    list
                    problemBanner
                    composer
                }
            }
            .navigationTitle("Ephemeral")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Model") { showingKey = true }
                        .tint(Palette.accent)
                }
            }
            .sheet(isPresented: $showingKey) { ModelSheet() }
        }
        .tint(Palette.accent)
        .preferredColorScheme(.dark)
        .onAppear(perform: reload)
    }

    private var list: some View {
        ScrollView {
            LazyVStack(spacing: 10) {
                if applications.isEmpty {
                    Text(
                        """
                        Nothing here yet.

                        Describe what you need below. Ephemeral records it — \
                        nothing is written or run until you generate it.
                        """
                    )
                    .foregroundStyle(Palette.inkQuiet)
                    .multilineTextAlignment(.center)
                    .padding(32)
                }

                ForEach(applications.filter { !$0.putAway }) { application in
                    NavigationLink(value: application.id) {
                        ApplicationCard(summary: application)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)
        }
        .navigationDestination(for: String.self) { ApplicationPage(id: $0) }
    }

    @ViewBuilder private var problemBanner: some View {
        if let problem {
            // Directly above the control that caused it. A failure rendered
            // where nobody is looking is the same as no failure, which is a
            // lesson the desktop window paid for in a film.
            Text(problem)
                .font(.footnote)
                .foregroundStyle(Palette.high)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
                .background(Palette.highSoft)
        }
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .bottom, spacing: 10) {
                TextField("What do you need?", text: $intent, axis: .vertical)
                    .lineLimit(1...4)
                    .textInputAutocapitalization(.sentences)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .background(Palette.ground, in: .rect(cornerRadius: 10))
                    .overlay(
                        RoundedRectangle(cornerRadius: 10).stroke(Palette.edgeStrong, lineWidth: 1)
                    )
                    .foregroundStyle(Palette.ink)

                // The one filled control on this screen, because asking for an
                // application grants nothing: no code is written and nothing
                // runs until somebody generates it, which is a separate act.
                Button("Create", action: create)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(Palette.accentInk)
                    .padding(.horizontal, 18)
                    .padding(.vertical, 12)
                    .background(Palette.accent, in: .rect(cornerRadius: 10))
                    .disabled(creating || intent.trimmingCharacters(in: .whitespaces).isEmpty)
            }

            // Said on the first screen rather than buried in an about box.
            // Somebody who taps Generate and waits for a build that will never
            // happen has been misled by this application, not by the engine.
            //
            // Deliberately *not* what Android says. The engine runs WebAssembly
            // applications on a device now (ADR-0021) and Android calls
            // `ephemeral_run`; this application does not yet. Claiming the
            // engine's capability here would be swapping one false sentence for
            // a newer one.
            Text(
                """
                Generating writes the source to this phone. This app does not run \
                what it generates yet — that happens on a computer.
                """
            )
            .font(.caption)
            .foregroundStyle(Palette.inkQuiet)
        }
        .padding(16)
        .background(Palette.surface)
    }

    private func create() {
        let asked = intent
        creating = true

        Engine.shared.submit({ try Engine.shared.create(intent: asked) }) { outcome in
            creating = false
            switch outcome {
            case .success:
                intent = ""
                problem = nil
                reload()
            case .failure(let error):
                problem = error.localizedDescription
            }
        }
    }

    private func reload() {
        Engine.shared.submit({ try Engine.shared.applications() }) { outcome in
            switch outcome {
            case .success(let found):
                applications = found
                problem = nil
            case .failure(let error):
                problem = error.localizedDescription
            }
        }
    }
}

/// One application, as a card.
///
/// Three facts drawn as three things. They were one grey subtitle joined by
/// dots, which meant an application allowed to reach the whole internet looked
/// exactly like an idle one that can see nothing of yours.
struct ApplicationCard: View {
    let summary: Summary

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(summary.purpose)
                .font(.headline)
                .foregroundStyle(Palette.ink)
                .multilineTextAlignment(.leading)

            Text(summary.name)
                .font(.caption)
                .foregroundStyle(Palette.inkQuiet)

            HStack(spacing: 8) {
                StatePill(state: summary.state, kind: summary.stateKind)

                // The only thing on the list that shouts, because it is the
                // only thing that cannot proceed without a person.
                if summary.awaitingDecision > 0 {
                    Text(
                        summary.awaitingDecision == 1
                            ? "1 decision waiting"
                            : "\(summary.awaitingDecision) decisions waiting"
                    )
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Palette.ground)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 3)
                    .background(Palette.high, in: .capsule)
                }
            }

            if summary.granted > 0 {
                Text(
                    summary.granted == 1 ? "Allowed 1 thing" : "Allowed \(summary.granted) things"
                )
                .font(.footnote)
                .foregroundStyle(Palette.forRisk(summary.highestGrantedRisk))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
        .background(Palette.surface, in: .rect(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Palette.edge, lineWidth: 1))
    }
}

/// Where an application is in its life. The colour is the lifecycle's answer.
struct StatePill: View {
    let state: String
    let kind: String

    var body: some View {
        HStack(spacing: 6) {
            Circle().frame(width: 6, height: 6)
            Text(state)
        }
        .font(.caption)
        .foregroundStyle(Palette.forState(kind))
        .padding(.horizontal, 12)
        .padding(.vertical, 3)
        .background(Palette.ground, in: .capsule)
        .overlay(Capsule().stroke(Palette.edge, lineWidth: 1))
    }
}

// MARK: - One application

struct ApplicationPage: View {
    let id: String

    @State private var page: Detail?
    @State private var status: String?
    @State private var generating = false
    @State private var confirming: Capability?

    var body: some View {
        ScrollView {
            if let page {
                VStack(alignment: .leading, spacing: 14) {
                    Text(page.summary.purpose)
                        .font(.title2.weight(.bold))
                        .foregroundStyle(Palette.ink)

                    StatePill(state: page.summary.state, kind: page.summary.stateKind)

                    Text(page.explanation).foregroundStyle(Palette.ink)
                    // Empty when it would only repeat the explanation. The
                    // service layer decides that once for every client; this
                    // screen only has to not draw an empty line.
                    if !page.description.isEmpty {
                        Text(page.description)
                            .font(.footnote)
                            .foregroundStyle(Palette.inkQuiet)
                    }

                    generateButton(page)

                    if let status {
                        Text(status)
                            .font(.footnote)
                            .foregroundStyle(Palette.medium)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(12)
                            .background(Palette.mediumSoft, in: .rect(cornerRadius: 10))
                    }

                    permissions(page)

                    Text("Kept: \(page.retention)")
                        .font(.footnote)
                        .foregroundStyle(Palette.inkQuiet)
                        .padding(.top, 8)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
            }
        }
        .background(Palette.ground)
        .navigationBarTitleDisplayMode(.inline)
        .onAppear(perform: load)
        .alert(item: $confirming) { capability in
            // A second, deliberate step for the requests that deserve one —
            // and which those are is the engine's judgement, not this screen's.
            Alert(
                title: Text(capability.wants),
                message: Text(capability.ifAllowed),
                primaryButton: .destructive(Text("Allow")) {
                    answer(capability, allow: true)
                },
                secondaryButton: .cancel()
            )
        }
    }

    @ViewBuilder private func generateButton(_ page: Detail) -> some View {
        Button(page.versions == 0 ? "Generate" : "Generate again") { write() }
            .font(.body.weight(.semibold))
            .foregroundStyle(Palette.accentInk)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 13)
            .background(Palette.accent, in: .rect(cornerRadius: 10))
            .disabled(generating)
    }

    @ViewBuilder private func permissions(_ page: Detail) -> some View {
        if !page.outstanding.isEmpty {
            heading("What it has asked for")
            ForEach(page.outstanding) { capability in
                PermissionCard(capability: capability, held: false) { allow in
                    if allow && capability.needsExplicitConfirmation {
                        confirming = capability
                    } else {
                        answer(capability, allow: allow)
                    }
                }
            }
        }

        if !page.allowed.isEmpty {
            heading("What it may do")
            ForEach(page.allowed) { capability in
                PermissionCard(capability: capability, held: true) { _ in }
            }
        }

        if page.isolated && page.outstanding.isEmpty {
            Text("This can see nothing of yours.")
                .font(.footnote)
                .foregroundStyle(Palette.low)
        }
    }

    private func heading(_ text: String) -> some View {
        Text(text.uppercased())
            .font(.caption.weight(.semibold))
            .kerning(0.8)
            .foregroundStyle(Palette.inkQuiet)
            .padding(.top, 10)
    }

    private func load() {
        Engine.shared.submit({ try Engine.shared.application(id) }) { outcome in
            switch outcome {
            case .success(let found): page = found
            case .failure(let error): status = error.localizedDescription
            }
        }
    }

    private func write() {
        guard Credential.present else {
            status = "Add a model key first — the button at the top of the list."
            return
        }

        generating = true
        status = "Planning and writing it. This can take a minute."

        Engine.shared.submit({
            if let key = Credential.read() { try Engine.shared.useCredential(key) }
            return try Engine.shared.generate(id)
        }) { outcome in
            generating = false
            switch outcome {
            case .success(let found):
                page = found
                status = nil
            case .failure(let error):
                status = error.localizedDescription
            }
        }
    }

    private func answer(_ capability: Capability, allow: Bool) {
        Engine.shared.submit({
            try Engine.shared.decide(id, capability: capability.capability, allow: allow)
        }) { outcome in
            switch outcome {
            case .success: load()
            case .failure(let error): status = error.localizedDescription
            }
        }
    }
}

/// One thing an application has asked for, or one it already holds.
///
/// Allow and Refuse are drawn identically, deliberately. A screen that made the
/// permissive answer the prettier one would be collecting consent rather than
/// asking for it.
struct PermissionCard: View {
    let capability: Capability
    let held: Bool
    let answer: (Bool) -> Void

    var body: some View {
        HStack(spacing: 0) {
            Rectangle()
                .fill(Palette.forRisk(capability.risk))
                .frame(width: 3)

            VStack(alignment: .leading, spacing: 6) {
                Text(held ? "✓ \(capability.wants)" : capability.wants)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Palette.ink)

                if let reason = capability.reason {
                    // Attributed on purpose: written by whatever asked, and the
                    // only part of a request a person cannot check.
                    Text("It says: \(reason)")
                        .font(.footnote)
                        .italic()
                        .foregroundStyle(Palette.inkQuiet)
                }

                // The consequence carries the risk colour. The reassurance
                // never does: "you can take this back" in crimson reads as a
                // warning and says the opposite of what it is.
                Text("\(capability.ifAllowed)  (\(capability.risk))")
                    .font(.footnote)
                    .foregroundStyle(Palette.forRisk(capability.risk))

                if !held {
                    HStack(spacing: 10) {
                        Button("Refuse") { answer(false) }
                        Button("Allow") { answer(true) }
                    }
                    .font(.subheadline)
                    .buttonStyle(.bordered)
                    .tint(Palette.inkQuiet)
                    .padding(.top, 4)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(14)
        }
        .background(Palette.surface, in: .rect(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Palette.edge, lineWidth: 1))
    }
}

// MARK: - Which model, and the key for it

/// One sheet, not two, because it is one decision: a key belongs to a
/// particular service, and choosing a service without saying which key goes
/// with it is how somebody sends an Anthropic key to Groq and gets an
/// authentication error that explains nothing.
struct ModelSheet: View {
    @Environment(\.dismiss) private var dismiss

    private let providers = Engine.providers()

    @State private var choice = Choice.stored ?? Choice(provider: "anthropic")
    @State private var key = ""
    @State private var said: String?
    @State private var checking = false
    @State private var reachable = true
    @State private var offered: [AvailableModel] = []

    private var chosen: Provider? {
        providers.first { $0.name == choice.provider }
    }

    var body: some View {
        NavigationStack {
            ZStack {
                Palette.ground.ignoresSafeArea()

                VStack(alignment: .leading, spacing: 16) {
                    // Built from what the engine has, never from a list written
                    // here: this application and the engine it links against do
                    // not ship on the same schedule, and a hardcoded list is
                    // wrong the first time a provider is added.
                    Picker("Service", selection: $choice.provider) {
                        ForEach(providers) { provider in
                            Text(provider.name).tag(provider.name)
                        }
                    }
                    .pickerStyle(.segmented)

                    if let chosen {
                        Text(chosen.what)
                            .font(.footnote)
                            .foregroundStyle(Palette.inkQuiet)

                        if chosen.configures("base_url") {
                            field("Base URL", placeholder: chosen.baseUrl ?? "", text: $choice.baseUrl)
                        }
                        if chosen.configures("model") {
                            field("Model", placeholder: chosen.model ?? "", text: $choice.model)
                        }
                        if chosen.needsCredential {
                            SecureField("API key", text: $key)
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                                .padding(14)
                                .background(Palette.surface, in: .rect(cornerRadius: 10))
                                .foregroundStyle(Palette.ink)

                            Text(
                                """
                                Kept in this phone's Keychain, sent only to the service \
                                above, and never written to Ephemeral's files or its log.
                                """
                            )
                            .font(.footnote)
                            .foregroundStyle(Palette.inkQuiet)
                        }

                        // The one control that can tell somebody the difference
                        // between configured and working. Before it existed,
                        // the first thing that could was a failed generation —
                        // after the intent had been sent and a bill had started.
                        Button(checking ? "Asking…" : "Check connection", action: check)
                            .disabled(checking)
                            .tint(Palette.accent)

                        // Offered rather than imposed: the box above stays
                        // typeable, because a service may know models it does
                        // not list, and a picker that refused an unlisted name
                        // would be a picker that decides what you may run.
                        if !offered.isEmpty, chosen.configures("model") {
                            Picker("Model", selection: $choice.model) {
                                Text("—").tag("")
                                ForEach(offered) { model in
                                    Text(model.shown).tag(model.name)
                                }
                            }
                            .pickerStyle(.menu)
                            .tint(Palette.accent)
                        }
                    }

                    if let said {
                        Text(said)
                            .font(.footnote)
                            .foregroundStyle(reachable ? Palette.low : Palette.critical)
                    }

                    Spacer()
                }
                .padding(16)
            }
            .navigationTitle("Model")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    // Offered next to the field that sets it: a key you cannot
                    // take back is a key you did not really lend.
                    Button("Forget key") {
                        Credential.forget()
                        said = "Key forgotten."
                        reachable = true
                        key = ""
                    }
                    .tint(Palette.inkQuiet)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Save", action: save).tint(Palette.accent)
                }
            }
        }
        .preferredColorScheme(.dark)
    }

    private func field(
        _ label: String,
        placeholder: String,
        text: Binding<String>
    ) -> some View {
        TextField(placeholder.isEmpty ? label : placeholder, text: text)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .keyboardType(.URL)
            .padding(14)
            .background(Palette.surface, in: .rect(cornerRadius: 10))
            .foregroundStyle(Palette.ink)
    }

    /// Asks the service what it has, which is also whether it can be reached.
    private func check() {
        checking = true
        said = nil

        // Saved first, and the key with it: the check has to test what
        // generation would do, which means the settings about to be used rather
        // than the ones the engine still holds.
        apply()

        Engine.shared.submit {
            try Engine.shared.models()
        } then: { outcome in
            checking = false
            switch outcome {
            case .success(let models):
                offered = models
                reachable = true
                said = models.isEmpty
                    ? "Reached it, and it listed no models."
                    : "Reached it. \(models.count) model\(models.count == 1 ? "" : "s")."
            // The service's own words. Ephemeral paraphrasing "invalid api key"
            // would be a worse sentence about a problem it cannot diagnose.
            case .failure(let why):
                reachable = false
                said = why.localizedDescription
            }
        }
    }

    private func save() {
        apply()
        Engine.shared.submit {} then: { _ in
            said = "Generating with \(choice.provider)."
            reachable = true
        }
    }

    /// Records the service and, if one was typed, the key.
    ///
    /// The service first. A key stored against a service nobody chose is a key
    /// sent to the wrong company on the very next generation.
    private func apply() {
        let typed = key.trimmed
        Engine.shared.submit {
            try Engine.shared.choose(choice)
            if !typed.isEmpty, Credential.save(typed) {
                try Engine.shared.useCredential(typed)
            }
        } then: { _ in }
    }
}
