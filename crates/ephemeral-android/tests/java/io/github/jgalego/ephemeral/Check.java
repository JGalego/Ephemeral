package io.github.jgalego.ephemeral;

import java.nio.file.Files;
import java.nio.file.Path;

/**
 * Drives the JNI bridge from a real JVM.
 *
 * There is no Android here and none is needed: JNI is JNI, and the part of this
 * bridge that is easy to get wrong — attaching a thread and calling back into
 * Java while native code waits — behaves the same in this process as it does in
 * an application's. What this cannot check is the application's own screens.
 *
 * Nothing here reaches the network. The transport is the test's, and it refuses
 * every request on purpose: a generation that fails still has to travel the
 * whole callback path to find out that it failed.
 */
public final class Check {
    private static int failures = 0;

    public static void main(String[] args) throws Exception {
        Path home = Files.createTempDirectory("ephemeral-jni");

        Recording refusing = new Recording(null);
        long session = Native.open(home.toString(), refusing);
        require(session != 0, "open returns a session");

        String created = Native.create(session, "compare two CSV files and show what differs");
        require(created != null, "create returns a summary");
        String id = field(created, "id");
        require(id != null && !id.isEmpty(), "the summary carries an id");

        String listed = Native.applications(session);
        require(listed != null && listed.contains(id), "the new application is in the list");

        String page = Native.application(session, id);
        require(page != null && page.contains("\"summary\""), "the page has a summary");

        // A failure has to arrive as a failure, with words, rather than as a
        // crash or a plausible-looking empty answer.
        require(Native.application(session, "not-an-id") == null, "an unknown id fails");
        require(Native.lastError(session) != null, "and the failure says why");

        // The callback path, which is the whole reason this bridge exists. The
        // transport refuses, so generation fails — but it must have been called,
        // with the credential and an https endpoint, and the process must still
        // be standing afterwards.
        require(Native.setCredential(session, "test-credential") == 0, "the credential is accepted");
        require(Native.generate(session, id) == null, "generation fails when the transport refuses");
        require(refusing.calls > 0, "generation went through the host transport");
        require(
            refusing.headers != null && refusing.headers.contains("test-credential"),
            "the transport was handed the credential, among the provider's own headers"
        );
        require(
            refusing.headers.contains("\"name\""),
            "as a header set rather than a bare key"
        );
        require(
            refusing.endpoint != null && refusing.endpoint.startsWith("https://"),
            "and an https endpoint"
        );
        require(refusing.body != null && refusing.body.contains("{"), "and a JSON body");
        require("POST".equals(refusing.method), "asking a model for something is a POST");
        require(Native.lastError(session) != null, "the refusal is reported");

        // Choosing a service. JNI is looked up by name and signature at call
        // time, so a method nothing exercises here is a method that fails on a
        // phone and nowhere else — which is the entire reason this harness
        // exists.
        require(
            Native.providers() != null && Native.providers().contains("openai"),
            "the catalogue crosses the bridge"
        );
        require(
            Native.setProvider(session, "{\"provider\":\"openai\","
                + "\"base_url\":\"https://api.groq.com/openai/v1\"}") == 0,
            "a service can be chosen from Java"
        );
        require(
            Native.provider(session) != null && Native.provider(session).contains("groq.com"),
            "and read back"
        );
        require(Native.generate(session, id) == null, "generating still fails, the transport refusing");
        require(
            refusing.endpoint != null && refusing.endpoint.contains("api.groq.com"),
            "but it went to the service that was chosen"
        );
        require(
            refusing.headers.contains("authorization"),
            "with that service's own header rather than the other one's"
        );

        // The connection test crosses the bridge too. The transport refuses, so
        // this fails — what matters is that the call went out and the refusal
        // came back as words rather than as a crash.
        require(Native.models(session) == null, "asking for models fails when the transport refuses");
        require(
            refusing.endpoint != null && refusing.endpoint.endsWith("/models"),
            "having asked the service where it lists them"
        );
        // The bug a real phone found: the method never crossed the bridge, so
        // this went out as a POST, OpenAI refused it with an empty body, and
        // the connection test reported a JSON parse error for a key that was
        // perfectly good.
        require("GET".equals(refusing.method), "and asked for it with a GET");
        require("".equals(refusing.body), "which carries no body at all");
        require(Native.lastError(session) != null, "and the refusal is reported");

        // Turning a filled-in form into a command crosses the bridge too. This
        // application declared nothing, so an empty form is an empty command —
        // what matters is that the call resolves and comes back as JSON rather
        // than failing to bind at run time, which is the one failure a desktop
        // JVM can catch and a phone cannot.
        require(
            "[]".equals(Native.arguments(session, id, "{}")),
            "a form with nothing in it is a command with nothing in it"
        );
        require(
            Native.arguments(session, "not-an-id", "{}") == null,
            "and an unknown application is refused rather than guessed at"
        );

        // Running crosses the bridge too. This application was created and
        // never generated, so there is nothing to run — what matters is that
        // the method binds by name and signature at call time and that the
        // refusal comes back as words. A method nothing exercises here is a
        // method that fails on a phone and nowhere else.
        require(
            Native.run(session, id, "[]") == null,
            "an application with nothing to run is refused"
        );
        require(
            Native.lastError(session) != null && Native.lastError(session).contains("generate"),
            "and is told what to do about it"
        );

        // A host whose transport throws is a host with a bug. It must not
        // become Ephemeral's crash: an exception left pending across a JNI
        // return poisons every later call.
        Throwing throwing = new Throwing();
        long second = Native.open(home.toString(), throwing);
        require(second != 0, "a second session opens");
        Native.setCredential(second, "test-credential");
        require(Native.generate(second, id) == null, "a throwing transport fails the call");
        require(throwing.calls > 0, "and was called before it threw");
        require(Native.applications(second) != null, "the session still works afterwards");
        Native.close(second);

        // Documented as allowed, and worth proving rather than assuming.
        Native.close(0);
        require(Native.create(0, "anything") == null, "a closed session refuses work");

        Native.close(session);

        System.out.println(failures == 0
            ? "  the bridge holds"
            : "  " + failures + " failed");
        System.exit(failures == 0 ? 0 : 1);
    }

    /** Pulls one string field out of JSON, without bringing a parser to a test. */
    private static String field(String json, String name) {
        if (json == null) {
            return null;
        }
        String key = "\"" + name + "\":\"";
        int at = json.indexOf(key);
        if (at < 0) {
            return null;
        }
        int from = at + key.length();
        int to = json.indexOf('"', from);
        return to < 0 ? null : json.substring(from, to);
    }

    private static void require(boolean held, String what) {
        System.out.println((held ? "  ok    " : "  FAIL  ") + what);
        if (!held) {
            failures++;
        }
    }

    /** A transport that answers with whatever it was told to, and remembers the ask. */
    private static final class Recording implements Transport {
        private final String reply;

        int calls;
        int reported;
        String method;
        String endpoint;
        String headers;
        String body;

        Recording(String reply) {
            this.reply = reply;
        }

        @Override
        public String send(String method, String endpoint, String headersJson, String body,
                           int[] status) {
            this.calls++;
            status[0] = this.reported;
            this.method = method;
            this.endpoint = endpoint;
            this.headers = headersJson;
            this.body = body;
            return reply;
        }
    }

    /** A transport with a bug in it. */
    private static final class Throwing implements Transport {
        int calls;

        @Override
        public String send(String method, String endpoint, String headersJson, String body,
                           int[] status) {
            this.calls++;
            throw new IllegalStateException("this host's transport is broken");
        }
    }
}
