# Android without Android Studio, for building apps from a terminal or an
# editor: the newest Java (Eclipse Temurin) in /usr/local/jdk, the newest
# Gradle in /usr/local/gradle, and Google's SDK command-line tools with adb,
# the newest platform and build tools in /usr/local/android-sdk (licences
# accepted). The SDK stays writable, so sdkmanager, or Gradle, adds whatever
# else a project asks for. Login shells get JAVA_HOME, ANDROID_HOME and PATH
# from /etc/profile.d; the desktop's terminals and editors through wrappers in
# /usr/local/bin (they aren't login shells). A phone connects over USB or
# `adb pair`/`adb connect`; there's no emulator (it would need /dev/kvm).
# JAVA_FEATURE (say 25), GRADLE_VERSION and ANDROID_CMDLINE_TOOLS_URL pin versions.
ANDROID_JDK=/usr/local/jdk
ANDROID_GRADLE=/usr/local/gradle
ANDROID_SDK=/usr/local/android-sdk
case "$(uname -m)" in
    x86_64) android_arch=x64 ;;
    aarch64) android_arch=aarch64 ;;
    *) echo "android: no Java for $(uname -m)" >&2; exit 1 ;;
esac

# Gradle first: the Java kept is the newest one it runs on.
if [[ -n "${GRADLE_VERSION:-}" ]]; then
    android_gradle_url="https://services.gradle.org/distributions/gradle-${GRADLE_VERSION}-bin.zip"
else
    android_gradle_url="$(curl -fsSL https://services.gradle.org/versions/current | python3 -c 'import json, sys; print(json.load(sys.stdin)["downloadUrl"])')"
fi
log "gradle: ${android_gradle_url}"
curl -fsSL -o /tmp/gradle.zip "${android_gradle_url}"
unzip -q /tmp/gradle.zip -d /tmp/gradle-unpacked
mv /tmp/gradle-unpacked/gradle-* "${ANDROID_GRADLE}"
rm -rf /tmp/gradle.zip /tmp/gradle-unpacked

# Java: Temurin's newest release, then each older one down to the newest LTS,
# until Gradle runs on it (a new Java can come out before Gradle supports it).
if [[ -n "${JAVA_FEATURE:-}" ]]; then
    android_javas="${JAVA_FEATURE}"
else
    android_javas="$(curl -fsSL https://api.adoptium.net/v3/info/available_releases | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(" ".join(str(v) for v in sorted(d["available_releases"], reverse=True) if d["most_recent_lts"] <= v <= d["most_recent_feature_release"]))')"
fi
android_java=""
for android_try in ${android_javas}; do
    log "java ${android_try}"
    rm -rf "${ANDROID_JDK}" && mkdir -p "${ANDROID_JDK}"
    curl -fsSL "https://api.adoptium.net/v3/binary/latest/${android_try}/ga/linux/${android_arch}/jdk/hotspot/normal/eclipse" \
        | tar -xz -C "${ANDROID_JDK}" --strip-components=1
    if JAVA_HOME="${ANDROID_JDK}" GRADLE_USER_HOME=/tmp/gradle-home "${ANDROID_GRADLE}/bin/gradle" --version > /dev/null 2>&1; then
        android_java="${android_try}"
        break
    fi
    log "gradle doesn't run on java ${android_try}"
done
rm -rf /tmp/gradle-home
[[ -n "${android_java}" ]] || { echo "android: gradle runs on none of java ${android_javas}" >&2; exit 1; }

# The SDK: the command-line tools (sdkmanager), then adb and the newest
# platform and build tools through them.
if [[ -z "${ANDROID_CMDLINE_TOOLS_URL:-}" ]]; then
    android_zip="$(curl -fsSL https://developer.android.com/studio | grep -oE 'commandlinetools-linux-[0-9]+_latest\.zip' | head -n1)"
    [[ -n "${android_zip}" ]] || { echo "could not find the SDK command-line tools; set ANDROID_CMDLINE_TOOLS_URL" >&2; exit 1; }
    ANDROID_CMDLINE_TOOLS_URL="https://dl.google.com/android/repository/${android_zip}"
fi
log "sdk command-line tools: ${ANDROID_CMDLINE_TOOLS_URL}"
curl -fsSL -o /tmp/cmdline-tools.zip "${ANDROID_CMDLINE_TOOLS_URL}"
unzip -q /tmp/cmdline-tools.zip -d /tmp/cmdline-tools-unpacked
mkdir -p "${ANDROID_SDK}/cmdline-tools"
mv /tmp/cmdline-tools-unpacked/cmdline-tools "${ANDROID_SDK}/cmdline-tools/latest"
rm -rf /tmp/cmdline-tools.zip /tmp/cmdline-tools-unpacked
android_sdkmanager() { JAVA_HOME="${ANDROID_JDK}" "${ANDROID_SDK}/cmdline-tools/latest/bin/sdkmanager" --sdk_root="${ANDROID_SDK}" "$@"; }
(yes || true) | android_sdkmanager --licenses > /dev/null
android_list="$(android_sdkmanager --list 2>/dev/null)"
# Released versions only: no betas, extension levels or release candidates.
# Platforms have a minor version since Android 16 (android-36.1, android-37.2).
android_platform="$(sed -nE 's/^ *(platforms;android-[0-9]+(\.[0-9]+)?) .*/\1/p' <<<"${android_list}" | sort -u -V | tail -n1)"
android_build_tools="$(sed -nE 's/^ *(build-tools;[0-9.]+) .*/\1/p' <<<"${android_list}" | sort -u -t';' -k2 -V | tail -n1)"
[[ -n "${android_platform}" && -n "${android_build_tools}" ]] || { echo "android: sdkmanager listed no platform or build tools" >&2; exit 1; }
log "sdk: platform-tools ${android_platform} ${android_build_tools}"
android_sdkmanager --install platform-tools "${android_platform}" "${android_build_tools}" > /dev/null
chmod -R a+rX "${ANDROID_JDK}" "${ANDROID_GRADLE}"
chmod -R a+rwX "${ANDROID_SDK}"

for android_bin in "${ANDROID_JDK}"/bin/* "${ANDROID_GRADLE}/bin/gradle" "${ANDROID_SDK}"/cmdline-tools/latest/bin/* \
    "${ANDROID_SDK}/platform-tools/adb" "${ANDROID_SDK}/platform-tools/fastboot"; do
    android_name="$(basename "${android_bin}")"
    if [[ -e "/usr/local/bin/${android_name}" ]]; then
        log "android: /usr/local/bin/${android_name} is the base's; not wrapping ${android_bin}"
        continue
    fi
    cat > "/usr/local/bin/${android_name}" <<WRAP
#!/bin/sh
export JAVA_HOME="\${JAVA_HOME:-${ANDROID_JDK}}" ANDROID_HOME="\${ANDROID_HOME:-${ANDROID_SDK}}"
exec ${android_bin} "\$@"
WRAP
    chmod 755 "/usr/local/bin/${android_name}"
done
cat > /etc/profile.d/wadspaces-android.sh <<PROFILE
export JAVA_HOME=${ANDROID_JDK}
export ANDROID_HOME=${ANDROID_SDK}
export PATH="${ANDROID_SDK}/cmdline-tools/latest/bin:${ANDROID_SDK}/platform-tools:${ANDROID_SDK}/build-tools/${android_build_tools#build-tools;}:${ANDROID_JDK}/bin:${ANDROID_GRADLE}/bin:\${PATH}"
PROFILE

# What's here, in a line (the package's version).
android_java_version="$("${ANDROID_JDK}/bin/java" -version 2>&1 | sed -nE '1s/.*version "([^"]+)".*/\1/p')"
android_gradle_version="$(basename "${android_gradle_url}" | sed -E 's/^gradle-(.*)-bin\.zip$/\1/')"
echo "Java ${android_java_version}, Gradle ${android_gradle_version}, Android ${android_platform#platforms;android-}, build-tools ${android_build_tools#build-tools;}" \
    > "${ANDROID_SDK}/wadspaces-versions"
cat "${ANDROID_SDK}/wadspaces-versions"
/usr/local/bin/adb version | head -n1
