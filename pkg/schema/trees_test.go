package schema

import "testing"

func ptr(s string) *string { return &s }

func TestBuildSpanTrees_Empty(t *testing.T) {
	// Empty input must return empty slice (not nil) so JSON marshals as [] not null.
	result := BuildSpanTrees(nil)
	if result == nil || len(result) != 0 {
		t.Fatalf("expected empty slice, got %v", result)
	}

	result = BuildSpanTrees([]SpanNode{})
	if result == nil || len(result) != 0 {
		t.Fatalf("expected empty slice, got %v", result)
	}
}

func TestBuildSpanTrees_SingleRoot(t *testing.T) {
	spans := []SpanNode{
		{SpanID: "root", Name: "GET /", Status: "OK"},
	}

	trees := BuildSpanTrees(spans)
	if len(trees) != 1 {
		t.Fatalf("expected 1 root, got %d", len(trees))
	}

	if trees[0].SpanID != "root" {
		t.Fatalf("expected root span, got %s", trees[0].SpanID)
	}

	if len(trees[0].Children) != 0 {
		t.Fatalf("expected no children, got %d", len(trees[0].Children))
	}
}

func TestBuildSpanTrees_ParentChild(t *testing.T) {
	spans := []SpanNode{
		{SpanID: "root", Name: "GET /", Status: "OK"},
		{SpanID: "child1", ParentID: ptr("root"), Name: "db.query", Status: "OK"},
		{SpanID: "child2", ParentID: ptr("root"), Name: "cache.get", Status: "OK"},
	}

	trees := BuildSpanTrees(spans)
	if len(trees) != 1 {
		t.Fatalf("expected 1 root, got %d", len(trees))
	}

	root := trees[0]
	if len(root.Children) != 2 {
		t.Fatalf("expected 2 children, got %d", len(root.Children))
	}

	names := map[string]bool{}
	for _, c := range root.Children {
		names[c.Name] = true
	}

	if !names["db.query"] || !names["cache.get"] {
		t.Fatalf("expected db.query and cache.get, got %v", names)
	}
}

func TestBuildSpanTrees_DeepNesting(t *testing.T) {
	spans := []SpanNode{
		{SpanID: "a", Name: "a", Status: "OK"},
		{SpanID: "b", ParentID: ptr("a"), Name: "b", Status: "OK"},
		{SpanID: "c", ParentID: ptr("b"), Name: "c", Status: "OK"},
	}

	trees := BuildSpanTrees(spans)
	if len(trees) != 1 {
		t.Fatalf("expected 1 root, got %d", len(trees))
	}

	if trees[0].SpanID != "a" {
		t.Fatal("root should be a")
	}

	if len(trees[0].Children) != 1 || trees[0].Children[0].SpanID != "b" {
		t.Fatal("a should have child b")
	}

	if len(trees[0].Children[0].Children) != 1 || trees[0].Children[0].Children[0].SpanID != "c" {
		t.Fatal("b should have child c")
	}
}

func TestBuildSpanTrees_MultipleRoots(t *testing.T) {
	spans := []SpanNode{
		{SpanID: "r1", Name: "trace1", Status: "OK"},
		{SpanID: "c1", ParentID: ptr("r1"), Name: "child1", Status: "OK"},
		{SpanID: "r2", Name: "trace2", Status: "ERROR"},
		{SpanID: "c2", ParentID: ptr("r2"), Name: "child2", Status: "ERROR"},
	}

	trees := BuildSpanTrees(spans)
	if len(trees) != 2 {
		t.Fatalf("expected 2 roots, got %d", len(trees))
	}
}

func TestBuildSpanTrees_OrphanParent(t *testing.T) {
	// Parent ID refers to a span not in this set — should be treated as root
	spans := []SpanNode{
		{SpanID: "orphan", ParentID: ptr("missing"), Name: "orphan", Status: "OK"},
		{SpanID: "child", ParentID: ptr("orphan"), Name: "child", Status: "OK"},
	}

	trees := BuildSpanTrees(spans)
	if len(trees) != 1 {
		t.Fatalf("expected 1 root (orphan), got %d", len(trees))
	}

	if trees[0].SpanID != "orphan" {
		t.Fatalf("expected orphan as root, got %s", trees[0].SpanID)
	}

	if len(trees[0].Children) != 1 {
		t.Fatalf("expected 1 child, got %d", len(trees[0].Children))
	}
}
