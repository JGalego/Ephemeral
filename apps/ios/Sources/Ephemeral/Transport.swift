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
    static let send: EphemeralHttpSend = { _, method, endpoint, headersJson, body in
        guard let method, let endpoint, let headersJson, let body,
              let url = URL(string: String(cString: endpoint))
        else { return nil }

        // The method the provider asked for, not an assumption. Listing what a
        // service has is a GET, and the version of this that always POSTed sent
        // that to `/v1/models` — which OpenAI refuses with an empty body, so
        // the connection test failed while generation worked.
        let verb = String(cString: method)
        var request = URLRequest(url: url)
        request.httpMethod = verb
        if verb == "POST" {
            request.httpBody = Data(String(cString: body).utf8)
        }

        // Exactly the headers the provider composed, and nothing added here.
        // Which headers a service wants is the provider's knowledge; this file
        // used to write Anthropic's three, which is why a phone could not be
        // pointed at any other service however it was configured.
        let headers = try? JSONSerialization.jsonObject(
            with: Data(String(cString: headersJson).utf8)
        )
        for header in headers as? [[String: String]] ?? [] {
            guard let name = header["name"], let value = header["value"] else { continue }
            request.setValue(value, forHTTPHeaderField: name)
        }

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
