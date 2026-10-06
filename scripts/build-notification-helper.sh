#!/bin/sh
set -eu

# 参数只控制构建产物与 Cargo 目标，不进入安装或通知权限流程。
if [ "$#" -ne 2 ]; then
    printf '%s\n' '用法：build-notification-helper.sh <输出可执行文件> <Cargo TARGET>' >&2
    exit 1
fi
notification_output=$1
case "$2" in
    aarch64-apple-darwin) notification_arch=arm64 ;;
    x86_64-apple-darwin) notification_arch=x86_64 ;;
    *) printf '%s\n' '不支持的 macOS 通知构建目标' >&2; exit 1 ;;
esac
notification_project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
notification_output_dir=$(dirname -- "$notification_output")
mkdir -p "$notification_output_dir"
notification_output_dir=$(CDPATH= cd -- "$notification_output_dir" && pwd)
notification_output="$notification_output_dir/$(basename -- "$notification_output")"
cd "$notification_project"
/usr/bin/xcrun swiftc -O -target "$notification_arch-apple-macosx11.0" \
    -module-cache-path "$notification_output_dir/swift-modules" \
    -module-name CodePerimeterNotifications \
    -framework AppKit -framework UserNotifications \
    native/notifications/CodePerimeterNotifications.swift -o "$notification_output"
chmod 700 "$notification_output"
