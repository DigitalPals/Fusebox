import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import java.io.FileInputStream
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

// Enable Firebase Google Services automatically when google-services.json is placed in android/app/.
// Builds without it (fresh sideloads / CI unit tests) still compile cleanly and fall back gracefully.
if (file("google-services.json").exists()) {
    apply(plugin = "com.google.gms.google-services")
}

val signingProps = Properties().apply {
    val file = listOfNotNull(
        System.getenv("FUSEBOX_ANDROID_SIGNING_FILE")?.let(::file),
        file("${System.getProperty("user.home")}/.config/fusebox-android/signing.properties"),
        rootProject.file("signing.properties"),
    ).firstOrNull { it.isFile }
    if (file != null) FileInputStream(file).use(::load)
}

fun signingValue(envKey: String, propKey: String): String? =
    System.getenv(envKey)?.takeIf { it.isNotBlank() }
        ?: signingProps.getProperty(propKey)?.takeIf { it.isNotBlank() }

android {
    namespace = "com.digitalpals.fusebox"
    compileSdk = 36

    val isPlayStore = (project.findProperty("fuseboxPlayStore") as String?)?.toBoolean() == true

    defaultConfig {
        applicationId = "com.digitalpals.fusebox"
        minSdk = 31
        targetSdk = 36
        versionCode = (project.findProperty("fuseboxVersionCode") as String?)?.toIntOrNull() ?: 3
        versionName = (project.findProperty("fuseboxVersionName") as String?) ?: "1.0.2"
        buildConfigField("boolean", "SELF_UPDATE_ENABLED", (!isPlayStore).toString())

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    if (isPlayStore) {
        sourceSets {
            getByName("release") {
                manifest.srcFile("src/playstore/AndroidManifest.xml")
            }
        }
    }

    val storeFilePath = signingValue("FUSEBOX_KEYSTORE_FILE", "storeFile")
    val storePass = signingValue("FUSEBOX_KEYSTORE_PASSWORD", "storePassword")
    val keyAliasName = signingValue("FUSEBOX_KEY_ALIAS", "keyAlias")
    val keyPass = signingValue("FUSEBOX_KEY_PASSWORD", "keyPassword")
    val hasReleaseSigning = storeFilePath != null && storePass != null && keyAliasName != null && keyPass != null

    signingConfigs {
        if (hasReleaseSigning) {
            create("release") {
                storeFile = file(storeFilePath!!)
                storePassword = storePass
                keyAlias = keyAliasName
                keyPassword = keyPass
                enableV1Signing = false
                enableV2Signing = true
                enableV3Signing = true
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            if (hasReleaseSigning) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
        debug {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }

    testOptions {
        unitTests {
            isReturnDefaultValues = true
            isIncludeAndroidResources = true
            all { test ->
                val dir = System.getProperty("fusebox.screenshots.dir")
                if (!dir.isNullOrBlank()) {
                    test.systemProperty("fusebox.screenshots.dir", dir)
                }
            }
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.05.01")
    implementation(composeBom)
    androidTestImplementation(composeBom)
    testImplementation(composeBom)

    implementation("androidx.core:core-ktx:1.17.0")
    implementation("androidx.activity:activity-compose:1.12.4")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.10.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")

    // Compose UI + Material 3 + Icons
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")

    // Adaptive / Foldable support (Jetpack WindowManager: FoldingFeature, posture, hinge bounds)
    implementation("androidx.window:window:1.5.1")

    // Networking & Serialization
    implementation("com.squareup.okhttp3:okhttp:5.1.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")

    // In-app browser (Custom Tabs) for dashboard sign-in / config
    implementation("androidx.browser:browser:1.9.0")

    // Home screen widgets (Jetpack Glance) & periodic refresh (WorkManager)
    implementation("androidx.glance:glance-appwidget:1.1.1")
    implementation("androidx.work:work-runtime-ktx:2.11.2")

    // Push notifications (FCM) — gracefully degrades if google-services.json is absent
    implementation("com.google.firebase:firebase-messaging:25.0.1")

    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
    testImplementation("com.squareup.okhttp3:mockwebserver3:5.1.0")
    testImplementation("org.robolectric:robolectric:4.16.1")
    testImplementation("androidx.compose.ui:ui-test-junit4")
}
