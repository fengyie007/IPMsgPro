package storage

// 对拍 tauri-rust storage.rs 的核心语义：bootstrap 定位、数据集身份校验、
// 重启时机迁移与失败回退。旧版目录兼容导入按"历史数据不迁移"决策不移植。

import (
	"os"
	"path/filepath"
	"testing"
)

func writeFile2(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o666); err != nil {
		t.Fatal(err)
	}
}

func TestOpenUsesBootstrapDirectlyWhenNoLocator(t *testing.T) {
	bootstrap := t.TempDir()
	writeFile2(t, filepath.Join(bootstrap, "config.json"), "{}")
	writeFile2(t, filepath.Join(bootstrap, "ipmsg.db"), "x")
	st, err := Open(bootstrap, DefaultPort)
	if err != nil {
		t.Fatal(err)
	}
	if !samePath(st.Active, bootstrap) {
		t.Fatalf("无定位文件时应直接用 bootstrap: %s", st.Active)
	}
	if !st.isDefaultLocation() {
		t.Fatal("bootstrap 应视为默认位置")
	}
}

func TestActiveDatasetRequiresValidMarker(t *testing.T) {
	bootstrap := t.TempDir()
	other := t.TempDir()
	// 标记缺失：定位文件指向的目录不可用 → 回退新默认位置并保留备份记录。
	if err := atomicJSON(filepath.Join(bootstrap, LocatorFile), locator{Active: other}); err != nil {
		t.Fatal(err)
	}
	st, err := Open(bootstrap, DefaultPort)
	if err != nil {
		t.Fatal(err)
	}
	if samePath(st.Active, other) {
		t.Fatalf("无效数据集不应被采用: %s", st.Active)
	}
	if st.errorText() == "" {
		t.Fatal("回退应产生提示")
	}
	entries, _ := os.ReadDir(bootstrap)
	foundBackup := false
	for _, e := range entries {
		if len(e.Name()) > 30 && e.Name()[:26] == "data-location-unavailable-" {
			foundBackup = true
		}
	}
	if !foundBackup {
		t.Fatal("原定位记录应被备份")
	}
	// 身份不符（complete=false 或端口不对）：致命错误，绝不混用。
	dataset, err := createDataset(t.TempDir()+"/dst", DefaultPort, "tok")
	if err != nil {
		t.Fatal(err)
	}
	dataset.keep = true
	if err := atomicJSON(filepath.Join(bootstrap, LocatorFile), locator{Active: dataset.path}); err != nil {
		t.Fatal(err)
	}
	if _, err := Open(bootstrap, DefaultPort); err == nil {
		t.Fatal("迁移未完成的数据集应报错")
	}
	if err := completeDataset(dataset); err != nil {
		t.Fatal(err)
	}
	if _, err := Open(bootstrap, DefaultPort); err != nil {
		t.Fatalf("补全标记后应可用: %v", err)
	}
}

func TestPendingMigrationAppliesOnNextOpen(t *testing.T) {
	bootstrap := t.TempDir()
	writeFile2(t, filepath.Join(bootstrap, "config.json"), `{"nickname":"迁移"}`)
	writeFile2(t, filepath.Join(bootstrap, "ipmsg.db"), "data")
	st, err := Open(bootstrap, DefaultPort)
	if err != nil {
		t.Fatal(err)
	}
	parent := t.TempDir()
	prepared, err := st.Prepare(parent, false)
	if err != nil {
		t.Fatal(err)
	}
	target := prepared["target"].(string)
	selection := prepared["selectionId"].(string)
	// Prepare 只登记，不复制。
	if _, err := os.Stat(target); err == nil {
		t.Fatal("prepare 阶段不应创建目标目录")
	}
	if _, err := st.Apply(selection); err != nil {
		t.Fatal(err)
	}
	// 本实例仍在原目录运行。
	if !samePath(st.Active, bootstrap) {
		t.Fatalf("apply 不应立即切换: %s", st.Active)
	}
	// 下一次启动完成迁移并切换。
	next, err := Open(bootstrap, DefaultPort)
	if err != nil {
		t.Fatal(err)
	}
	if !samePath(next.Active, target) {
		t.Fatalf("重启后应切到新数据集: %s", next.Active)
	}
	data, err := os.ReadFile(filepath.Join(next.Active, "config.json"))
	if err != nil || string(data) != `{"nickname":"迁移"}` {
		t.Fatalf("迁移应复制配置: %q %v", data, err)
	}
	if _, err := os.Stat(filepath.Join(next.Active, MarkerFile)); err != nil {
		t.Fatal("迁移结果应有完成标记")
	}
	// 取消路径：再次 apply 后 cancel 回原状。
	p2, err := next.Prepare(parent, false)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := next.Apply(p2["selectionId"].(string)); err != nil {
		t.Fatal(err)
	}
	info, err := next.CancelPending()
	if err != nil {
		t.Fatal(err)
	}
	if info["pending"] != nil {
		t.Fatalf("取消后不应有 pending: %v", info["pending"])
	}
}

func TestPrepareRejectsInvalidTargets(t *testing.T) {
	bootstrap := t.TempDir()
	st, err := Open(bootstrap, DefaultPort)
	if err != nil {
		t.Fatal(err)
	}
	// 目标位于当前数据目录内。
	if _, err := st.Prepare(filepath.Join(st.Active, "sub"), false); err == nil {
		t.Fatal("数据目录内部应被拒绝")
	}
	// 文件路径不是文件夹。
	if _, err := st.Prepare(filepath.Join(bootstrap, "config.json"), false); err == nil {
		t.Fatal("文件应被拒绝")
	}
	// 迁移目标不能在源内。
	if _, err := migrate(st.Active, &Choice{Token: "t", Target: filepath.Join(st.Active, "x")}, DefaultPort); err == nil {
		t.Fatal("迁移目标在源内应拒绝")
	}
}

func TestDefaultDirectoryNaming(t *testing.T) {
	if got := DirectoryName(DefaultPort); got != "SpeedIpMsg" {
		t.Fatalf("默认端口目录名异常: %q", got)
	}
	if got := DirectoryName(2426); got != "SpeedIpMsg-2426" {
		t.Fatalf("非默认端口目录名异常: %q", got)
	}
}
