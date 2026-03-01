package schema

// BuildSpanTrees assembles flat spans into parent-child trees.
// It indexes spans by SpanID, identifies roots (spans whose ParentID
// is nil or not present in the set), and recursively builds children.
func BuildSpanTrees(spans []SpanNode) []SpanNode {
	if len(spans) == 0 {
		return nil
	}

	// Index by ID (pointer into input slice) and collect children lists.
	childrenOf := make(map[string][]string, len(spans))

	byID := make(map[string]*SpanNode, len(spans))
	for i := range spans {
		spans[i].Children = nil
		byID[spans[i].SpanID] = &spans[i]
	}

	// Build parent→children index and identify roots
	var rootIDs []string

	for i := range spans {
		s := &spans[i]
		if s.ParentID != nil {
			if _, ok := byID[*s.ParentID]; ok {
				childrenOf[*s.ParentID] = append(childrenOf[*s.ParentID], s.SpanID)
				continue
			}
		}

		rootIDs = append(rootIDs, s.SpanID)
	}

	// Recursively build tree from leaves up
	var build func(id string) SpanNode

	build = func(id string) SpanNode {
		node := *byID[id]
		for _, childID := range childrenOf[id] {
			node.Children = append(node.Children, build(childID))
		}

		return node
	}

	result := make([]SpanNode, 0, len(rootIDs))
	for _, id := range rootIDs {
		result = append(result, build(id))
	}

	return result
}
