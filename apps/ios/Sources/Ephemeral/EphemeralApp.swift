// The application itself, which is four lines and one decision.
//
// The decision is `.dark`: Ephemeral is dark on every platform, and the reason
// is written down once, in `crates/ephemeral-design` — a tool for watching what
// software on your machine is allowed to do should not flash white at somebody
// twenty times a day. Unlike the desktop window there is no switch here yet;
// when there is one, it belongs in the same place the key does.

import SwiftUI

@main
struct EphemeralApp: App {
    var body: some Scene {
        WindowGroup {
            ApplicationList()
        }
    }
}
