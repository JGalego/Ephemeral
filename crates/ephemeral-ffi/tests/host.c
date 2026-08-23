/*
 * A host, in C, doing exactly what a phone does.
 *
 * The Rust tests in `src/lib.rs` drive the same functions, but they do it from
 * Rust: they can only string-match the header, and they link the crate as an
 * rlib. This links the real static library against the real header and calls
 * through the C ABI, which is the only way to find out whether the two actually
 * agree — a declaration that has drifted from its export is a compile or link
 * error here and nothing at all over there.
 *
 * Swift and Kotlin both reach Ephemeral through C. If this works, the boundary
 * they will use works; what is left for them is a user interface, not a
 * contract.
 *
 * Deliberately no test framework: a dependency here would be a dependency in
 * the thing being tested.
 */

#include <assert.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "ephemeral.h"

/* ---------------------------------------------------------------- the host */

/* What this fake host replies with, in order. */
static const char *const PLAN_REPLY =
    "{\"content\":[{\"type\":\"text\",\"text\":\""
    "{\\\"name\\\":\\\"Word Counter\\\","
    "\\\"summary\\\":\\\"counts the words in a file\\\","
    "\\\"runtime\\\":\\\"docker\\\",\\\"image\\\":\\\"python:3.12-slim\\\","
    "\\\"interface\\\":\\\"command_line\\\","
    "\\\"requests\\\":[{\\\"capability\\\":\\\"filesystem_read\\\","
    "\\\"target\\\":\\\"~/Downloads/**\\\","
    "\\\"reason\\\":\\\"to read the file you pick\\\"}]}"
    "\"}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}";

static const char *const GENERATE_REPLY =
    "{\"content\":[{\"type\":\"text\",\"text\":\""
    "{\\\"files\\\":[{\\\"path\\\":\\\"main.py\\\","
    "\\\"contents\\\":\\\"print('hi')\\\"}],"
    "\\\"dockerfile\\\":\\\"FROM python:3.12-slim\\\","
    "\\\"entrypoint\\\":[\\\"python\\\",\\\"/app/main.py\\\"],"
    "\\\"test_command\\\":[\\\"true\\\"]}"
    "\"}],\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}";

/*
 * The same two, framed the way everything that copied OpenAI frames a reply.
 * A second service is not a second endpoint with the same envelope: the whole
 * shape differs, which is why a provider owns the parsing and the transport
 * owns none of it.
 */
static const char *const OPENAI_PLAN_REPLY =
    "{\"choices\":[{\"message\":{\"content\":\""
    "{\\\"name\\\":\\\"Word Counter\\\","
    "\\\"summary\\\":\\\"counts the words in a file\\\","
    "\\\"runtime\\\":\\\"docker\\\",\\\"image\\\":\\\"python:3.12-slim\\\","
    "\\\"interface\\\":\\\"command_line\\\",\\\"requests\\\":[]}"
    "\"}}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}";

static const char *const OPENAI_GENERATE_REPLY =
    "{\"choices\":[{\"message\":{\"content\":\""
    "{\\\"files\\\":[{\\\"path\\\":\\\"main.py\\\","
    "\\\"contents\\\":\\\"print('hi')\\\"}],"
    "\\\"dockerfile\\\":\\\"FROM python:3.12-slim\\\","
    "\\\"entrypoint\\\":[\\\"python\\\",\\\"/app/main.py\\\"],"
    "\\\"test_command\\\":[\\\"true\\\"]}"
    "\"}}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4}}";

/* Everything the host owns, handed to Ephemeral as an opaque context. */
struct host {
  int calls;
  int frees;
  /* Whether to answer in the OpenAI envelope rather than Anthropic's. */
  bool openai;
  /* Where the last request went, and with which headers. */
  char endpoint[512];
  char headers[2048];
};

/*
 * The HTTPS transport. On a phone this is URLSession or OkHttp; here it answers
 * from a script. Either way Ephemeral never opens a socket.
 */
static char *host_send(void *context, const char *endpoint,
                       const char *headers_json, const char *request_json) {
  struct host *host = context;

  /* The contract says these are readable for the duration of the call. */
  assert(endpoint != NULL);
  assert(headers_json != NULL);
  assert(request_json != NULL && strstr(request_json, "\"model\"") != NULL);

  /* Recorded rather than judged. Which endpoint and which headers are right
     depends on which service was chosen, and that is the thing under test —
     a transport that asserted either would be a transport that knew which
     provider it belonged to, which is the bug this ABI change removed. */
  snprintf(host->endpoint, sizeof(host->endpoint), "%s", endpoint);
  snprintf(host->headers, sizeof(host->headers), "%s", headers_json);

  const char *reply;
  if (host->openai) {
    reply = (host->calls % 2 == 0) ? OPENAI_PLAN_REPLY : OPENAI_GENERATE_REPLY;
  } else {
    reply = (host->calls == 0) ? PLAN_REPLY : GENERATE_REPLY;
  }
  host->calls += 1;

  /* Our allocation, released through our own free function. */
  char *owned = malloc(strlen(reply) + 1);
  assert(owned != NULL);
  strcpy(owned, reply);
  return owned;
}

static void host_free(void *context, char *response) {
  struct host *host = context;
  host->frees += 1;
  free(response);
}

/* --------------------------------------------------------------- the tests */

static int failures = 0;

static void check(bool condition, const char *what) {
  if (condition) {
    printf("  ok  %s\n", what);
  } else {
    printf("FAIL  %s\n", what);
    failures += 1;
  }
}

/* Reads a returned string, then releases it the way the contract requires. */
static bool contains(char *owned, const char *needle) {
  if (owned == NULL) {
    return false;
  }
  bool found = strstr(owned, needle) != NULL;
  ephemeral_string_free(owned);
  return found;
}

int main(int argc, char **argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: %s <home-directory>\n", argv[0]);
    return 2;
  }

  struct host host = {.calls = 0, .frees = 0, .openai = false};

  EphemeralHandle *handle =
      ephemeral_open(argv[1], host_send, host_free, &host);
  check(handle != NULL, "a host can open Ephemeral through the C ABI");
  if (handle == NULL) {
    return 1;
  }

  check(ephemeral_set_credential(handle, "sk-test-not-a-real-key") ==
            EPHEMERAL_OK,
        "a credential arrives from the platform's secure store, not the environment");

  /* Creating needs no credential and no network — which is why a phone can. */
  char *created = ephemeral_create(handle, "count the words in a file");
  check(created != NULL, "an application can be created on the device");
  if (created == NULL) {
    return 1;
  }

  /* Pull the id out without a JSON parser: this is a smoke test, not a client. */
  char id[128] = {0};
  const char *marker = strstr(created, "\"id\":\"");
  check(marker != NULL, "the summary comes back as JSON");
  if (marker == NULL) {
    return 1;
  }
  marker += strlen("\"id\":\"");
  const char *end = strchr(marker, '"');
  size_t length = (size_t)(end - marker);
  assert(length < sizeof(id));
  memcpy(id, marker, length);
  ephemeral_string_free(created);

  check(contains(ephemeral_applications(handle), id),
        "it appears in the list");

  /* The whole point: generating on the device, through the host's own HTTPS. */
  char *generated = ephemeral_generate(handle, id);
  check(generated != NULL, "an application is generated through the host's transport");
  if (generated == NULL) {
    char *reason = ephemeral_last_error(handle);
    fprintf(stderr, "generate failed: %s\n", reason ? reason : "(no reason)");
    ephemeral_string_free(reason);
    return 1;
  }

  check(strstr(generated, "\"granted\":0") != NULL,
        "generating grants nothing — a phone cannot widen what an app may do");
  check(strstr(generated, "filesystem_read") != NULL,
        "what it asked for is recorded as a request");
  ephemeral_string_free(generated);

  check(host.calls == 2, "exactly one plan and one generate crossed the boundary");
  check(host.frees == host.calls,
        "every response the host allocated was handed back to be freed");

  /* A permission cannot be composed out of a string the host invents. */
  check(ephemeral_decide(handle, id, "network_outbound", true) == EPHEMERAL_ERROR,
        "deciding something never requested is refused");
  check(contains(ephemeral_last_error(handle), "network_outbound"),
        "and the refusal says which capability it was");

  check(ephemeral_decide(handle, id, "filesystem_read", true) == EPHEMERAL_OK,
        "deciding something actually requested is recorded");

  /* Both halves of the permission model reach a phone as well (ADR-0003): the
     person allowed the application, and Ephemeral itself has not been allowed
     to carry it out — because nothing on a device mirrors the operating
     system's own permissions into the ledger yet. So the decision stands and
     does nothing, which is what the page has to say. Asserting "granted: 1"
     here would be asserting authority the sandbox would not give. */
  /* `contains` takes ownership, so the page is asked for once per question
     rather than freed twice — which is the sort of thing this test exists to
     catch in the ABI itself. */
  check(contains(ephemeral_application(handle, id), "\"granted\":0"),
        "and it can use nothing yet, because Ephemeral may not carry it out");
  check(contains(ephemeral_application(handle, id), "\"effective\":false"),
        "which the page says outright rather than by omission");
  check(contains(ephemeral_application(handle, id), "filesystem_read"),
        "while the decision itself is still recorded as theirs");


  /* ------------------------------------------------ and now somewhere else */

  /*
   * The same host, pointed at Groq. This is the whole of ADR-0020 in one
   * block: nothing about the transport changed, nothing was recompiled, and
   * the request goes somewhere else carrying somebody else's headers.
   *
   * Before that change this was not expressible. The callback took an
   * `api_key`, this file wrote `x-api-key` and `anthropic-version` around it,
   * and no configuration anywhere could have produced a request to any other
   * service.
   */
  check(ephemeral_set_provider(handle,
                               "{\"provider\":\"openai\","
                               "\"base_url\":\"https://api.groq.com/openai/v1\","
                               "\"model\":\"llama-3.3-70b-versatile\"}") == EPHEMERAL_OK,
        "a host can choose a different service entirely");

  check(contains(ephemeral_provider(handle), "groq.com"),
        "and read back what it chose");

  host.openai = true;
  host.calls = 0;

  char *second = ephemeral_create(handle, "count the lines in a file");
  check(second != NULL, "a second application, to generate with it");
  if (second == NULL) {
    return 1;
  }

  char other[128] = {0};
  const char *at = strstr(second, "\"id\":\"");
  assert(at != NULL);
  at += strlen("\"id\":\"");
  const char *stop = strchr(at, '"');
  size_t width = (size_t)(stop - at);
  assert(width < sizeof(other));
  memcpy(other, at, width);
  ephemeral_string_free(second);

  char *elsewhere = ephemeral_generate(handle, other);
  check(elsewhere != NULL, "it generates through the same host transport");
  if (elsewhere == NULL) {
    char *reason = ephemeral_last_error(handle);
    fprintf(stderr, "generate failed: %s\n", reason ? reason : "(no reason)");
    ephemeral_string_free(reason);
    return 1;
  }
  ephemeral_string_free(elsewhere);

  check(strstr(host.endpoint, "api.groq.com") != NULL,
        "the request went to the service that was chosen");
  /* Lower case as sent. HTTP header names are case-insensitive and a real
     host should treat them so; this is checking the bytes that crossed. */
  check(strstr(host.headers, "authorization") != NULL &&
            strstr(host.headers, "Bearer") != NULL,
        "carrying the credential the way that service wants it");
  check(strstr(host.headers, "x-api-key") == NULL,
        "and not the way the other one does");

  /* A name nobody has is refused, and the previous choice stands. Quietly
     falling back would send the next generation to a company the person did
     not pick. */
  check(ephemeral_set_provider(handle, "{\"provider\":\"gorq\"}") == EPHEMERAL_ERROR,
        "a provider that does not exist is refused");
  check(contains(ephemeral_provider(handle), "groq.com"),
        "and the choice that was already made is left alone");

  check(contains(ephemeral_providers(), "anthropic"),
        "the catalogue a picker is built from needs no handle at all");

  /* A host that gets this wrong should get a code, not undefined behaviour. */
  check(ephemeral_decide(NULL, id, "filesystem_read", true) == EPHEMERAL_BAD_HANDLE,
        "a null handle is refused rather than dereferenced");
  ephemeral_close(NULL);
  ephemeral_string_free(NULL);

  ephemeral_close(handle);

  printf("\n%s\n", failures == 0 ? "The C ABI holds." : "The C ABI is broken.");
  return failures == 0 ? 0 : 1;
}
