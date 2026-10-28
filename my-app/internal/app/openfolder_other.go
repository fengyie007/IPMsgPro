//go:build !windows

package app

import "fmt"

func revealInExplorer(path string) error {
	return fmt.Errorf("当前平台暂不支持打开文件夹")
}
