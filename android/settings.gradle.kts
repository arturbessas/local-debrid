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

rootProject.name = "LocalDebrid"
include(":app")
include(":rustls-platform-verifier")
project(":rustls-platform-verifier").projectDir = file("third_party/rustls-platform-verifier")
