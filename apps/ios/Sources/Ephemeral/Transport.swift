// The HTTPS this application performs on the engine's behalf.
//
// Ephemeral does not open sockets and brings no HTTP stack (ADR-0017). That is
// not a limitation being worked around: it is what makes generating on iOS
// possible at all. The desktop transport hands a request to `curl`, and iOS
// does not let an application spawn a process — so the host supplies the
// transport, and TLS, certificate policy and background behaviour stay with the
// platform that already has opinions about them.
//
// Everything the model is sent leaves this file. Nothing else in the
// application makes a network request.

import Ephemeral
import Foundation

enum Transport {

    /// Sends one request, and blocks until it answers.
    ///
    /// A C function pointer cannot capture anything, so this is a plain
    /// closure with no context — everything it needs arrives as an argument.
    /// Blocking is correct here: the engine's call is synchronous, and it is
    /// already running off the main thread on the engine's own queue.
    static let send: EphemeralHttpSend = { _, endpoint, apiKey, body in
        guard let endpoint, let apiKey, let body,
              let url = URL(string: String(cString: endpoint))
        else { return nil }

        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue(String(cString: apiKey), forHTTPHeaderField: "x-api-key")
        request.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        request.httpBody = Data(String(cString: body).utf8)

        // Generation is a model request and takes as long as one takes. The
        // default sixty seconds is not enough, and a timeout in the middle of
        // writing an application reads to a person as the application failing.
        request.timeoutInterval = 300

        let waiting = DispatchSemaphore(value: 0)
        var reply: String?

        URLSession.shared.dataTask(with: request) { data, _, _ in
            reply = data.flatMap { String(data: $0, encoding: .utf8) }
            waiting.signal()
        }.resume()

        waiting.wait()

        // `strdup`, because the allocation is ours throughout: Ephemeral copies
        // what it needs immediately and then hands this back to `release`.
        return reply.map { strdup($0) } ?? nil
    }

    /// Frees what `send` returned. The other half of owning the allocation.
    static let release: EphemeralHttpFree = { _, response in
        free(response)
    }
}
