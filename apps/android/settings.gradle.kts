// The Android application is its own build, for the same reason the desktop
// window is: it needs the Android SDK, and everything else in this repository
// must stay buildable and testable on a machine that does not have one.

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "ephemeral-android"
include(":app")
