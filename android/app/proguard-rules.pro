# Keep kotlinx.serialization generated serializers and model classes
-keepattributes *Annotation*, InnerClasses
-dontnote kotlinx.serialization.AnnotationsKt
-keepclassmembers class kotlinx.serialization.json.** {
    *** Companion;
}
-keepclasseswithmembers class kotlinx.serialization.json.** {
    kotlinx.serialization.KSerializer serializer(...);
}
-keep,includedescriptorclasses class com.digitalpals.fusebox.data.**$$serializer { *; }
-keepclassmembers class com.digitalpals.fusebox.data.** {
    *** Companion;
}
-keepclasseswithmembers class com.digitalpals.fusebox.data.** {
    kotlinx.serialization.KSerializer serializer(...);
}
# OkHttp optional platform classes
-dontwarn okhttp3.**
-dontwarn okio.**
-dontwarn org.conscrypt.**
-dontwarn org.bouncycastle.**
-dontwarn org.openjsse.**
