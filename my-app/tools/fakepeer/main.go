// fakepeer：黑盒验证 demo 可执行文件的最小对端模拟器。
// 流程：向目标实例单播 BR_ENTRY → 等 ANSENTRY → 发 SENDMSG(中文, GBK) → 等 RECVMSG 回执。
// 全部步骤通过则退出码 0；任一步超时退出码 1。
//
// 用法：go run ./tools/fakepeer -target=127.0.0.1:2435 [-name=FakePeer]
package main

import (
	"bytes"
	"flag"
	"fmt"
	"net"
	"os"
	"strconv"
	"time"

	"golang.org/x/text/encoding/simplifiedchinese"
)

func gbk(s string) []byte {
	b, err := simplifiedchinese.GBK.NewEncoder().Bytes([]byte(s))
	if err != nil {
		return []byte(s)
	}
	return b
}

func buildPacket(pktNo uint32, username, hostname string, command uint32, body string, extra string) []byte {
	var buf bytes.Buffer
	fmt.Fprintf(&buf, "1:%d:%s:%s:%d:", pktNo, gbk(username), gbk(hostname), command)
	buf.Write(gbk(body))
	buf.WriteByte(0)
	if extra != "" {
		buf.Write(gbk(extra))
		buf.WriteByte(0)
	}
	return buf.Bytes()
}

func main() {
	target := flag.String("target", "127.0.0.1:2435", "demo 实例地址 ip:port")
	name := flag.String("name", "FakePeer", "本端昵称")
	flag.Parse()

	addr, err := net.ResolveUDPAddr("udp4", *target)
	if err != nil {
		fmt.Println("FAIL 解析目标地址:", err)
		os.Exit(1)
	}
	conn, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4zero, Port: 0})
	if err != nil {
		fmt.Println("FAIL 绑定 UDP:", err)
		os.Exit(1)
	}
	defer conn.Close()

	var pktNo uint32 = 1000
	send := func(command uint32, body, extra string) {
		pktNo++
		if _, err := conn.WriteToUDP(buildPacket(pktNo, *name, "fakehost", command, body, extra), addr); err != nil {
			fmt.Println("FAIL 发送:", err)
			os.Exit(1)
		}
	}

	recv := func(timeout time.Duration) (net.Addr, []byte, bool) {
		conn.SetReadDeadline(time.Now().Add(timeout))
		buf := make([]byte, 65507)
		n, from, err := conn.ReadFromUDP(buf)
		if err != nil {
			return nil, nil, false
		}
		return from, buf[:n], true
	}

	// 1) BR_ENTRY：body=昵称，extra=分组
	send(0x01, *name, "FakeGroup")
	from, data, ok := recv(3 * time.Second)
	if !ok {
		fmt.Println("FAIL 未收到 ANSENTRY（demo 未应答发现）")
		os.Exit(1)
	}
	fmt.Printf("ANSENTRY 来自 %s: %q\n", from, summarize(data))

	// 2) SENDMSG | SENDCHECKOPT，目标地址用 ANSENTRY 的来源
	addr = from.(*net.UDPAddr)
	text := "你好，来自 FakePeer 的中文消息"
	send(0x20|0x100, text, "")
	_, data2, ok := recv(3 * time.Second)
	if !ok {
		fmt.Println("FAIL 未收到 RECVMSG 回执")
		os.Exit(1)
	}
	parts := bytes.SplitN(data2, []byte{':'}, 6)
	if len(parts) != 6 || string(parts[4]) != strconv.Itoa(0x21) {
		fmt.Printf("FAIL 回执报文异常: %q\n", summarize(data2))
		os.Exit(1)
	}
	body := parts[5]
	if i := bytes.IndexByte(body, 0); i >= 0 {
		body = body[:i]
	}
	ackNo, err := strconv.ParseUint(string(body), 10, 32)
	if err != nil || uint32(ackNo) != pktNo {
		fmt.Printf("FAIL 回执包号不匹配: got %s want %d\n", body, pktNo)
		os.Exit(1)
	}
	fmt.Printf("RECVMSG 回执正确（包号 %d）\n", pktNo)
	fmt.Println("PASS fakepeer 黑盒验证通过：发现应答 + 文本回执")
}

// summarize 把报文压成可读单行（GBK 段保留原字节以十六进制展示太长，直接截断展示）。
func summarize(data []byte) string {
	if len(data) > 120 {
		data = data[:120]
	}
	return string(data)
}
