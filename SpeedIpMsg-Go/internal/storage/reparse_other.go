//go:build !windows

package storage

import "os"

func reparsePoint(meta os.FileInfo) bool {
	return meta.Mode()&os.ModeSymlink != 0
}
