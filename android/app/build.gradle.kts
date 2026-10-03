import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val repoRoot = rootProject.projectDir.parentFile
val keystoreProps = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
val rustAbis = (findProperty("rustAbis") as String? ?: "arm64-v8a,x86_64").split(",")

android {
    namespace = "com.localdebrid.app"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.localdebrid.app"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "1.0.0"
        ndk { abiFilters += rustAbis }
    }

    signingConfigs {
        create("release") {
            if (keystoreProps.isNotEmpty()) {
                storeFile = rootProject.file(keystoreProps.getProperty("storeFile"))
                storePassword = keystoreProps.getProperty("storePassword")
                keyAlias = keystoreProps.getProperty("keyAlias")
                keyPassword = keystoreProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = if (keystoreProps.isNotEmpty()) {
                signingConfigs.getByName("release")
            } else {
                signingConfigs.getByName("debug")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        jniLibs { useLegacyPackaging = false }
    }
}

kotlin { jvmToolchain(17) }

dependencies {
    implementation(project(":rustls-platform-verifier"))
}

// Builds the Rust server (crates/localdebrid-jni) into src/main/jniLibs.
// Pass -PskipRust=true to reuse libraries built separately (e.g. by scripts/build-android.sh).
val cargoBuild by tasks.registering(Exec::class) {
    val release = gradle.startParameter.taskNames.any { it.contains("Release", ignoreCase = true) }
    workingDir = repoRoot
    val args = mutableListOf("cargo", "ndk", "--platform", "26", "-o", "android/app/src/main/jniLibs")
    rustAbis.forEach { args += listOf("-t", it) }
    args += listOf("build", "-p", "localdebrid-jni")
    if (release) args += "--release"
    commandLine(args)
    onlyIf { findProperty("skipRust") != "true" }
}

tasks.named("preBuild") { dependsOn(cargoBuild) }
