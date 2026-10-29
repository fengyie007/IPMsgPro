//go:build !windows

package main

// showErrorBox 在非 Windows 平台只输出到 stderr（终端可见）。
func showErrorBox(title, text string) {}

// requireWebView 由各平台自行保证 webview 可用，这里不拦。
func requireWebView() error { return nil }
