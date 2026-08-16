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

/* Everything the host owns, handed to Ephemeral as an opaque context. */
struct host {
  int calls;
  int frees;
};

/*
 * The HTTPS transport. On a phone this is URLSession or OkHttp; here it answers
 * from a script. Either way Ephemeral never opens a socket.
 */
static char *host_send(void *context, const char *endpoint, const char *api_key,
                       const char *request_json) {
  struct host *host = context;

  /* The contract says these are readable for the duration of the call. */
  assert(endpoint != NULL && strstr(endpoint, "anthropic.com") != NULL);
  assert(api_key != NULL && strncmp(api_key, "sk-", 3) == 0);
  assert(request_json != NULL && strstr(request_json, "\"model\"") != NULL);

  const char *reply = (host->calls == 0) ? PLAN_REPLY : GENERATE_REPLY;
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

  struct host host = {.calls = 0, .frees = 0};

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
  check(contains(ephemeral_application(handle, id), "\"granted\":1"),
        "and the application now holds it");

  /* A host that gets this wrong should get a code, not undefined behaviour. */
  check(ephemeral_decide(NULL, id, "filesystem_read", true) == EPHEMERAL_BAD_HANDLE,
        "a null handle is refused rather than dereferenced");
  ephemeral_close(NULL);
  ephemeral_string_free(NULL);

  ephemeral_close(handle);

  printf("\n%s\n", failures == 0 ? "The C ABI holds." : "The C ABI is broken.");
  return failures == 0 ? 0 : 1;
}
