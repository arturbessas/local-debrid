// Android half of rustls-platform-verifier v0.7.1 (MIT). Keep in sync with the
// rustls-platform-verifier version in Cargo.lock.
plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "org.rustls.platformverifier"
    compileSdk = 36
    defaultConfig {
        minSdk = 26
        buildConfigField("boolean", "TEST", "false")
        consumerProguardFiles("proguard-rules.pro")
    }
    buildFeatures { buildConfig = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin { jvmToolchain(17) }
