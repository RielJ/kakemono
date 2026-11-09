// Command wireclient sends raw request bytes to a wallpaperd Unix socket and
// writes the raw response bytes, unmodified, to a file. It is used by
// tools/goldens/replay.sh to capture IPC wire goldens.
//
//	wireclient -socket S -req request.jsonl -resp response.jsonl
//
// It writes the whole request file in one write, half-closes the connection
// (so the Go server's line scanner sees EOF after the last line), then reads
// until the server closes the connection.
package main

import (
	"flag"
	"io"
	"log"
	"net"
	"os"
	"time"
)

func main() {
	socket := flag.String("socket", "", "daemon socket path")
	reqPath := flag.String("req", "", "file holding the raw request bytes")
	respPath := flag.String("resp", "", "file to write the raw response bytes to")
	flag.Parse()
	if *socket == "" || *reqPath == "" || *respPath == "" {
		log.Fatal("usage: wireclient -socket S -req FILE -resp FILE")
	}

	req, err := os.ReadFile(*reqPath)
	if err != nil {
		log.Fatal(err)
	}

	conn, err := net.DialTimeout("unix", *socket, 3*time.Second)
	if err != nil {
		log.Fatal(err)
	}
	defer conn.Close()
	if err := conn.SetDeadline(time.Now().Add(30 * time.Second)); err != nil {
		log.Fatal(err)
	}

	if _, err := conn.Write(req); err != nil {
		log.Fatal(err)
	}
	if err := conn.(*net.UnixConn).CloseWrite(); err != nil {
		log.Fatal(err)
	}

	resp, err := io.ReadAll(conn)
	if err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*respPath, resp, 0o644); err != nil {
		log.Fatal(err)
	}
}
