plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Signing, and why there is no key in this repository.
//
// Android will not install an unsigned APK at all — unlike macOS and Windows,
// where an unsigned build merely warns. So a release build has to be signed
// with something, and a signing key is a credential, which SECURITY.md says
// does not live in the tree. It is read from the environment instead: CI
// supplies one, and a local `assembleRelease` without it produces an unsigned
// APK that you can inspect but not install.
//
// Until a real key lives in repository secrets, the one CI uses is generated
// per release and thrown away. That is honest but has a consequence worth
// stating: Android refuses to upgrade an installed app when the new signature
// differs, so a new release must be uninstalled and reinstalled rather than
// updated in place. See apps/android/README.md.
val keystorePath: String? = System.getenv("EPHEMERAL_ANDROID_KEYSTORE")

// The version comes from the release, not from a number kept in step by hand.
// Two places claiming to know the version is one place being wrong.
val releaseVersion: String = System.getenv("EPHEMERAL_VERSION")?.removePrefix("v")
    ?: "0.1.0"

// Android orders releases by an integer, and it must only ever increase.
// 1.2.3 becomes 10203, which stays ordered as long as minor and patch stay
// under 100 — checked here rather than discovered by a store rejecting an
// upgrade.
val releaseCode: Int = releaseVersion
    .substringBefore('-')
    .split('.')
    .map { it.toIntOrNull() ?: 0 }
    .let { parts ->
        val major = parts.getOrElse(0) { 0 }
        val minor = parts.getOrElse(1) { 0 }
        val patch = parts.getOrElse(2) { 0 }
        require(minor < 100 && patch < 100) {
            "$releaseVersion cannot be turned into an increasing versionCode"
        }
        major * 10_000 + minor * 100 + patch
    }

android {
    namespace = "io.github.jgalego.ephemeral"
    compileSdk = 34

    defaultConfig {
        applicationId = "io.github.jgalego.ephemeral"
        minSdk = 26
        targetSdk = 34
        versionCode = releaseCode
        versionName = releaseVersion
    }

    if (keystorePath != null) {
        signingConfigs {
            create("release") {
                storeFile = file(keystorePath)
                storePassword = System.getenv("EPHEMERAL_ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("EPHEMERAL_ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("EPHEMERAL_ANDROID_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    // The .so files are built by ../build-native.sh, not by Gradle. Keeping
    // Gradle out of the cross-compilation means the Rust build is the same
    // command here and in CI, and a broken NDK fails where you can read it.
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }

    // No dependencies on purpose. Ephemeral's engine is the only thing this app
    // needs; everything else it uses is in the platform. That keeps the APK
    // small, the build hermetic, and the list of third parties with code on
    // your phone as short as it can honestly be.
}
