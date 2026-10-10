plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

// Single version source: the workspace Cargo.toml. versionName and the
// hello frame's client version (V2Client.VERSION via BuildConfig) both
// derive from it, so the surfaces cannot drift apart again.
val workspaceVersion: String =
    file("../../../Cargo.toml").readText()
        .substringAfter("[workspace.package]")
        .let { Regex("(?m)^version\\s*=\\s*\"([^\"]+)\"").find(it) }
        ?.groupValues?.get(1)
        ?: throw GradleException("missing [workspace.package] version in the workspace Cargo.toml")

android {
    namespace = "app.pulpit.mobile"
    compileSdk = 34

    defaultConfig {
        applicationId = "app.pulpit.mobile"
        // the target tablet (SM-T561, LineageOS) runs Android 7.1 (API 25)
        minSdk = 24
        targetSdk = 34
        versionCode = 4
        versionName = workspaceVersion
    }

    buildTypes {
        release {
            // R8: kotlinx-serialization and OkHttp ship their consumer
            // keep rules; the debug signing config lets this install
            // straight over the debug build on the deck tablet.
            isMinifyEnabled = true
            isShrinkResources = true
            signingConfig = signingConfigs.getByName("debug")
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        compose = true
        // exposes VERSION_NAME to V2Client.VERSION (single version source)
        buildConfig = true
    }
    // Golden fixtures shared with the Rust contract tests
    // (crates/proto/tests/fixtures) so both stacks parse the same wire
    // examples.
    sourceSets.getByName("test") {
        resources.srcDir("../../../crates/proto/tests/fixtures")
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.09.02")
    implementation(composeBom)

    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.activity:activity-compose:1.9.2")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.6")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.6")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")

    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.2")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")

    testImplementation("junit:junit:4.13.2")
}
