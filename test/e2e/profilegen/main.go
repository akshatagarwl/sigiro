// profilegen sends synthetic OTLP profiles to an OTel collector via gRPC.
// telemetrygen does not support profiles (open-telemetry/opentelemetry-collector-contrib#36127
// closed as not-planned), so we need a custom tool.
package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"math/rand/v2"
	"os"
	"os/signal"
	"strings"
	"time"

	"go.opentelemetry.io/collector/pdata/pcommon"
	"go.opentelemetry.io/collector/pdata/pprofile"
	"go.opentelemetry.io/collector/pdata/pprofile/pprofileotlp"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

func main() {
	endpoint := flag.String("endpoint", "localhost:4317", "collector gRPC endpoint")
	insecureConn := flag.Bool("insecure", false, "use insecure gRPC connection")
	services := flag.String("services", "checkout,payments", "comma-separated service names")
	duration := flag.Duration("duration", 30*time.Second, "how long to generate profiles")
	rate := flag.Float64("rate", 2, "profiles per second per service")

	flag.Parse()

	if !*insecureConn {
		log.Fatal("--insecure is required (TLS not implemented)")
	}

	svcList := strings.Split(*services, ",")
	if len(svcList) == 0 {
		log.Fatal("--services must not be empty")
	}

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt)
	defer cancel()

	conn, err := grpc.NewClient(*endpoint, grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		log.Fatalf("grpc dial: %v", err)
	}
	defer conn.Close()

	client := pprofileotlp.NewGRPCClient(conn)

	interval := time.Duration(float64(time.Second) / *rate)

	ticker := time.NewTicker(interval)
	defer ticker.Stop()

	deadline := time.After(*duration)
	sent := 0

	log.Printf("profilegen: sending to %s, services=%v, rate=%.0f/s, duration=%s", *endpoint, svcList, *rate, *duration)

	for {
		select {
		case <-ctx.Done():
			log.Printf("profilegen: interrupted, sent %d profiles", sent)
			return
		case <-deadline:
			log.Printf("profilegen: done, sent %d profiles", sent)
			return
		case <-ticker.C:
			for _, svc := range svcList {
				pd := buildProfile(svc)

				req := pprofileotlp.NewExportRequestFromProfiles(pd)
				if _, err := client.Export(ctx, req); err != nil {
					log.Printf("export error: %v", err)
					continue
				}

				sent++
			}
		}
	}
}

// Function name pool — realistic Go stdlib + app functions.
var funcNames = []string{
	"runtime.gcBgMarkWorker",           // 0
	"runtime.gcDrain",                  // 1
	"runtime.mallocgc",                 // 2
	"net/http.(*conn).serve",           // 3
	"net/http.serverHandler.ServeHTTP", // 4
	"main.handleRequest",               // 5
	"database/sql.(*DB).QueryContext",  // 6
	"database/sql.(*DB).query",         // 7
	"encoding/json.(*Encoder).Encode",  // 8
	"runtime.goexit",                   // 9
	"runtime.systemstack",              // 10
	"runtime.gcMarkTermination",        // 11
	"runtime.(*mcache).nextFree",       // 12
	"github.com/lib/pq.(*conn).Query",  // 13
	"cpu",                              // 14
	"nanoseconds",                      // 15
}

// Stack definitions as slices of indices into funcNames.
var stacks = [][]int32{
	{9, 0, 1},              // GC drain path
	{10, 11},               // GC mark termination
	{9, 3, 4, 5, 6, 7, 13}, // HTTP → DB query
	{9, 3, 4, 5, 8},        // HTTP → JSON encode
	{9, 3, 5, 2, 12},       // malloc/allocation
}

func buildProfile(service string) pprofile.Profiles {
	pd := pprofile.NewProfiles()
	dict := pd.Dictionary()

	// StringTable
	for _, s := range funcNames {
		dict.StringTable().Append(s)
	}

	// FunctionTable: 1:1 with funcNames
	for i := range funcNames {
		fn := dict.FunctionTable().AppendEmpty()
		fn.SetNameStrindex(int32(i))
	}

	// LocationTable: 1:1 with functions, each with one Line
	for i := range funcNames {
		loc := dict.LocationTable().AppendEmpty()
		line := loc.Lines().AppendEmpty()
		line.SetFunctionIndex(int32(i))
	}

	// StackTable
	for _, stack := range stacks {
		st := dict.StackTable().AppendEmpty()
		for _, locIdx := range stack {
			st.LocationIndices().Append(locIdx)
		}
	}

	// ResourceProfiles — service.name is required (mapper.go panics if missing)
	rp := pd.ResourceProfiles().AppendEmpty()
	res := rp.Resource()
	res.Attributes().PutStr("service.name", service)
	res.Attributes().PutStr("host.name", fmt.Sprintf("%s-host", service))

	// ScopeProfiles
	sp := rp.ScopeProfiles().AppendEmpty()
	sp.Scope().SetName("profilegen")

	// Profile
	p := sp.Profiles().AppendEmpty()
	p.SampleType().SetTypeStrindex(14) // "cpu"
	p.SampleType().SetUnitStrindex(15) // "nanoseconds"
	p.SetTime(pcommon.Timestamp(time.Now().UnixNano()))

	// 3-5 samples with varying stack indices and values
	numSamples := 3 + rand.IntN(3)
	for i := range numSamples {
		s := p.Samples().AppendEmpty()
		s.SetStackIndex(int32(i % len(stacks)))
		s.Values().Append(int64(50 + rand.IntN(500)))
	}

	return pd
}
