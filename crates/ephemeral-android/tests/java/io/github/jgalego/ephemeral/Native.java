package io.github.jgalego.ephemeral;

/**
 * The engine's functions, declared as the application declares them.
 *
 * This mirrors {@code apps/android/app/src/main/java/…/Native.kt}. A JNI symbol
 * is derived from the class and method name, so this file being called
 * {@code Native} in this package is what makes it bind to the same exports the
 * Android application binds to: a name or signature that drifts here fails the
 * same way it would fail on a phone, which is the point of testing it here.
 *
 * Kotlin's {@code object} produces instance methods and these are static. That
 * difference is invisible across JNI — the second argument is a {@code jclass}
 * rather than a {@code jobject}, and the bridge dereferences neither.
 */
final class Native {
    static {
        System.loadLibrary("ephemeral_android");
    }

    static native long open(String home, Transport transport);

    static native void close(long session);

    static native int setCredential(long session, String apiKey);

    static native int setProvider(long session, String configuration);

    static native String provider(long session);

    static native String providers();

    static native String models(long session);

    static native String lastError(long session);

    static native String create(long session, String intent);

    static native String applications(long session);

    static native String application(long session, String id);

    static native String generate(long session, String id);

    static native int decide(long session, String id, String capability, boolean allow);

    private Native() {
    }
}
