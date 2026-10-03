use monty_analysis::{AnalysisLimits, AnalysisOutcome, AnalysisRequest, Graph, Node, Span, analyze};

fn request(source: &str, expression: &str, stubs: Option<&str>) -> AnalysisRequest {
    let start = u32::try_from(source.rfind(expression).unwrap()).unwrap();
    AnalysisRequest {
        source: source.into(),
        stubs: stubs.map(str::to_owned),
        targets: vec![Span {
            start,
            end: start + u32::try_from(expression.len()).unwrap(),
        }],
        limits: AnalysisLimits::default(),
    }
}
fn infer(source: &str, expression: &str, stubs: Option<&str>) -> Graph {
    match analyze(&request(source, expression, stubs)).unwrap().outcome {
        AnalysisOutcome::Inferred(graph) => graph,
        other @ AnalysisOutcome::Rejected(_) => panic!("expected inference: {other:?}"),
    }
}
fn root(graph: &Graph) -> &Node {
    &graph.nodes[graph.roots[0].0 as usize]
}
fn instance(node: &Node, name: &str) {
    assert!(
        matches!(node, Node::Instance { identity, .. } if identity.path.last().unwrap() == name),
        "{node:?}"
    );
}

#[test]
fn nullable_coalescing_and_method_membership_are_python() {
    let expression = "'grocery' in (description or '').lower()";
    let source = format!("def check(description: str | None) -> bool:\n    return {expression}\n");
    instance(root(&infer(&source, expression, None)), "bool");
    let expression = "value or 'missing'";
    let graph = infer(
        &format!("def f(value: int):\n    return {expression}\n"),
        expression,
        None,
    );
    assert!(matches!(root(&graph), Node::Union(_)), "{graph:?}");
    assert!(
        graph
            .nodes
            .iter()
            .any(|n| matches!(n, Node::StringLiteral(s) if s == "missing"))
    );
}

#[test]
fn tuples_literals_and_nested_containers_preserve_structure() {
    let graph = infer("value = (1, True, b'x', ['a'])", "(1, True, b'x', ['a'])", None);
    assert!(matches!(root(&graph), Node::Tuple { prefix, variable: None, .. } if prefix.len() == 4));
    assert!(graph.nodes.contains(&Node::IntLiteral(1)));
    assert!(graph.nodes.contains(&Node::BoolLiteral(true)));
    assert!(graph.nodes.contains(&Node::BytesLiteral(b"x".to_vec())));
    assert!(graph.nodes.iter().any(|n| matches!(n, Node::Instance { identity, arguments } if identity.path.last().unwrap() == "list" && arguments.len() == 1)));
}

#[test]
fn record_presence_readonly_union_and_nominal_are_retained() {
    let stubs = "from typing import TypedDict, NotRequired, ReadOnly, NewType\nSongId = NewType('SongId', int)\nclass Row(TypedDict):\n    id: SongId\n    description: NotRequired[ReadOnly[str | None]]\ndef row() -> Row: ...\n";
    let graph = infer("value = row()", "row()", Some(stubs));
    let Node::Record { fields, identity, .. } = root(&graph) else {
        panic!("{graph:?}")
    };
    assert!(identity.is_some());
    let field = fields.iter().find(|f| f.name == "description").unwrap();
    assert!(!field.required);
    assert!(field.read_only);
    assert!(matches!(graph.nodes[field.ty.0 as usize], Node::Union(_)));
    assert!(
        graph
            .nodes
            .iter()
            .any(|n| matches!(n, Node::NewType { identity, .. } if identity.path[0] == "SongId"))
    );
    assert!(graph.nodes.contains(&Node::None));
}

#[test]
fn recursion_is_a_graph_not_truncation() {
    let stubs = "from typing import TypedDict, NotRequired\nclass Tree(TypedDict):\n    children: NotRequired[list[Tree]]\ndef tree() -> Tree: ...\n";
    let graph = infer("tree()", "tree()", Some(stubs));
    assert!(graph.nodes.len() < 10, "{graph:?}");
    assert!(!graph.nodes.contains(&Node::Unsupported));
    let Node::Record { fields, .. } = root(&graph) else {
        panic!("{graph:?}")
    };
    let Node::Instance { arguments, .. } = &graph.nodes[fields[0].ty.0 as usize] else {
        panic!("{graph:?}")
    };
    assert_eq!(arguments, &graph.roots);
}

#[test]
fn analysis_never_executes_or_resolves_host_calls() {
    let source =
        "def explode() -> int:\n    raise RuntimeError('must not run')\nvalue = explode()\nwhile True:\n    pass\n";
    instance(root(&infer(source, "explode()", None)), "int");
    instance(root(&infer("host()", "host()", Some("def host() -> str: ..."))), "str");
}

#[test]
fn callers_and_failed_requests_are_isolated() {
    infer("host()", "host()", Some("def host() -> int: ..."));
    let bad = analyze(&request("host()", "host()", None)).unwrap();
    assert!(matches!(bad.outcome, AnalysisOutcome::Rejected(_)));
    instance(root(&infer("host()", "host()", Some("def host() -> str: ..."))), "str");
}

#[test]
fn diagnostics_use_original_utf8_offsets_with_stubs() {
    let source = "label = 'é'\nmissing_name\n";
    let result = analyze(&request(source, "missing_name", Some("x: int"))).unwrap();
    let AnalysisOutcome::Rejected(diagnostics) = result.outcome else {
        panic!()
    };
    let d = diagnostics
        .iter()
        .find(|d| d.code.contains("unresolved-reference"))
        .unwrap();
    let span = d.span.unwrap();
    assert_eq!(&source[span.start as usize..span.end as usize], "missing_name");
}

#[test]
fn any_unknown_and_unsupported_remain_distinct() {
    assert_eq!(
        root(&infer("value", "value", Some("from typing import Any\nvalue: Any"))),
        &Node::Any
    );
    assert_eq!(root(&infer("def f(x):\n    return x\n", "x", None)), &Node::Unknown);
    assert_eq!(
        root(&infer("def f() -> int:\n    return 1\nf\n", "f", None)),
        &Node::Unsupported
    );
}

#[test]
fn nested_function_span_exports_its_checked_body_return() {
    let source = "def outer(xs: list[int]):\n    def doubled(items: list[int]):\n        return [x * 2 for x in items]\n    return doubled(xs)\n";
    let nested = "def doubled(items: list[int]):\n        return [x * 2 for x in items]";
    let start = u32::try_from(source.find(nested).unwrap()).unwrap();
    let result = monty_analysis::analyze_function_at(
        &AnalysisRequest {
            source: source.into(),
            stubs: Some("marker: int".into()),
            targets: vec![],
            limits: AnalysisLimits::default(),
        },
        Span {
            start,
            end: start + u32::try_from(nested.len()).unwrap(),
        },
    )
    .unwrap();
    let AnalysisOutcome::Inferred(graph) = result.outcome else {
        panic!("{result:?}")
    };
    assert!(!graph.nodes.contains(&Node::Unknown), "{graph:?}");
    assert!(
        matches!(root(&graph), Node::Instance { identity, .. } if identity.path.last().unwrap() == "list"),
        "{graph:?}"
    );
}

#[test]
fn limits_and_nonexpression_targets_fail_without_partial_success() {
    let mut req = request("(1, 2)", "(1, 2)", None);
    req.limits.max_type_references = 1;
    assert!(analyze(&req).unwrap_err().contains("graph limit"));
    req.limits = AnalysisLimits::default();
    req.targets[0].end -= 1;
    assert!(analyze(&req).unwrap_err().contains("match an expression"));
    req.limits.max_source_bytes = 1;
    assert!(analyze(&req).unwrap_err().contains("byte limit"));
}

#[test]
fn arithmetic_index_comprehension_datetime_and_multiple_targets() {
    let source = "from datetime import datetime, timedelta\ndef f(xs: list[int], now: datetime):\n    doubled = [x * 2 for x in xs]\n    return (doubled[0], now + timedelta(days=1))\n";
    let mut req = request(source, "doubled[0]", None);
    req.targets
        .extend(request(source, "now + timedelta(days=1)", None).targets);
    let AnalysisOutcome::Inferred(graph) = analyze(&req).unwrap().outcome else {
        panic!()
    };
    instance(&graph.nodes[graph.roots[0].0 as usize], "int");
    instance(&graph.nodes[graph.roots[1].0 as usize], "datetime");
}

#[test]
fn truthiness_refinements_and_nominal_bases_are_not_erased() {
    for expression in ["value or 'missing'", "value and 2", "value if value else 2"] {
        let graph = infer(
            &format!("def f(value: str | None):\n    return {expression}\n"),
            expression,
            None,
        );
        assert!(!graph.nodes.contains(&Node::Unsupported), "{graph:?}");
        assert!(!graph.nodes.contains(&Node::Unknown), "{graph:?}");
    }
    let graph = infer(
        "value",
        "value",
        Some("from typing import NewType\nBase = NewType('Base', int)\nChild = NewType('Child', Base)\nvalue: Child"),
    );
    let Node::NewType { base, .. } = root(&graph) else {
        panic!("{graph:?}")
    };
    assert!(matches!(&graph.nodes[base.0 as usize], Node::NewType { identity, .. } if identity.path[0] == "Base"));
}

#[test]
fn recursive_aliases_and_nominal_source_locations_survive() {
    let graph = infer("value", "value", Some("type Tree = int | list[Tree]\nvalue: Tree"));
    assert!(graph.nodes.len() < 10);
    assert!(!graph.nodes.contains(&Node::Unsupported));
    let source = "class Token:\n    pass\nvalue = Token()";
    let graph = infer(source, "Token()", Some("extra: int"));
    let Node::Instance { identity, .. } = root(&graph) else {
        panic!("{graph:?}")
    };
    assert_eq!(identity.source, "/analysis.py");
    assert_eq!(
        identity.start,
        Some(u32::try_from(source.find("Token").unwrap()).unwrap())
    );
}

#[test]
fn containers_tuple_segments_and_scoped_nominals_keep_their_types() {
    for ty in [
        "dict[str, list[tuple[int | None, bytes]]]",
        "set[int]",
        "frozenset[str]",
        "tuple[int, *tuple[str, ...], bytes]",
    ] {
        let graph = infer("value", "value", Some(&format!("value: {ty}")));
        assert!(!graph.nodes.contains(&Node::Unknown), "{ty}: {graph:?}");
        assert!(!graph.nodes.contains(&Node::Unsupported), "{ty}: {graph:?}");
    }
    let stubs = "class A:\n    class Token: ...\nclass B:\n    class Token: ...\na: A.Token\nb: B.Token\n";
    let graph = infer("(a, b)", "(a, b)", Some(stubs));
    let Node::Tuple { prefix, .. } = root(&graph) else {
        panic!("{graph:?}")
    };
    let Node::Instance { identity: a, .. } = &graph.nodes[prefix[0].0 as usize] else {
        panic!("{graph:?}")
    };
    let Node::Instance { identity: b, .. } = &graph.nodes[prefix[1].0 as usize] else {
        panic!("{graph:?}")
    };
    assert_ne!(a, b);
}

#[test]
fn empty_targets_check_definitions_without_execution() {
    let mut req = request(
        "raise RuntimeError('not executed')",
        "RuntimeError('not executed')",
        None,
    );
    req.targets.clear();
    let AnalysisOutcome::Inferred(graph) = analyze(&req).unwrap().outcome else {
        panic!()
    };
    assert!(graph.roots.is_empty());
    assert!(graph.nodes.is_empty());
    req.source = "def f() -> int:\n    return 'wrong'".into();
    assert!(matches!(analyze(&req).unwrap().outcome, AnalysisOutcome::Rejected(_)));
}

#[test]
fn branch_evidence_uses_python_control_flow() {
    use monty_analysis::{Binding, BranchRequest, analyze_branches};
    for predicate in [
        "value is not None",
        "not (value is None)",
        "value is not None and value > 0",
    ] {
        let request = BranchRequest {
            bindings: vec![Binding {
                name: "value".into(),
                annotation: "int | None".into(),
            }],
            predicate: predicate.into(),
            subjects: vec!["value".into()],
            imports: String::new(),
            stubs: None,
            limits: AnalysisLimits::default(),
        };
        let AnalysisOutcome::Inferred(graph) = analyze_branches(&request).unwrap().outcome else {
            panic!("branch rejected")
        };
        instance(&graph.nodes[graph.roots[0].0 as usize], "int");
        if predicate == "value is not None" {
            assert_eq!(graph.nodes[graph.roots[1].0 as usize], Node::None);
        }
    }
    let request = BranchRequest {
        bindings: vec![Binding {
            name: "value".into(),
            annotation: "str | int".into(),
        }],
        predicate: "isinstance(value, str)".into(),
        subjects: vec!["value".into()],
        imports: String::new(),
        stubs: None,
        limits: AnalysisLimits::default(),
    };
    let AnalysisOutcome::Inferred(graph) = analyze_branches(&request).unwrap().outcome else {
        panic!("branch rejected")
    };
    instance(&graph.nodes[graph.roots[0].0 as usize], "str");
    instance(&graph.nodes[graph.roots[1].0 as usize], "int");
}

#[test]
fn branch_evidence_checks_expression_boundaries_and_budgets() {
    use monty_analysis::{Binding, BranchRequest, analyze_branches};
    let mut request = BranchRequest {
        bindings: vec![Binding {
            name: "value".into(),
            annotation: "int | None".into(),
        }],
        predicate: "value is None".into(),
        subjects: vec!["value".into()],
        imports: String::new(),
        stubs: None,
        limits: AnalysisLimits::default(),
    };
    let AnalysisOutcome::Inferred(graph) = analyze_branches(&request).unwrap().outcome else {
        panic!("branch rejected")
    };
    assert_eq!(graph.nodes[graph.roots[0].0 as usize], Node::None);
    instance(&graph.nodes[graph.roots[1].0 as usize], "int");
    request.limits.max_targets = 1;
    assert!(analyze_branches(&request).is_err());
    request.limits = AnalysisLimits::default();
    request.subjects[0] = "value); injected = 1; (value".into();
    assert!(analyze_branches(&request).is_err());
}

#[test]
fn standard_descriptors_preserve_factory_return_types() {
    let stubs = "from typing import Self\nclass Base:\n    @classmethod\n    def create(cls) -> Self: ...\n    @staticmethod\n    def count() -> int: ...\nclass Child(Base): ...\n";
    for (expression, name) in [("Child.create()", "Child"), ("Child.count()", "int")] {
        instance(
            root(&infer(&format!("value = {expression}"), expression, Some(stubs))),
            name,
        );
    }
    for (expression, name) in [
        ("datetime.fromisoformat('2024-01-01T00:00:00+00:00')", "datetime"),
        ("date.fromisoformat('2024-01-01')", "date"),
        ("time.fromisoformat('12:00:00')", "time"),
    ] {
        instance(
            root(&infer(
                &format!("from datetime import datetime, date, time\nvalue = {expression}"),
                expression,
                None,
            )),
            name,
        );
    }
}

#[test]
fn chained_guards_refine_reused_values_without_leaking() {
    let expression = "None is not value < cutoff";
    let source = format!("def f(value: int | None, cutoff: int):\n    return {expression}\n");
    instance(root(&infer(&source, expression, None)), "bool");
    for expression in ["None is value < cutoff", "value < cutoff"] {
        let source = format!("def f(value: int | None, cutoff: int):\n    return {expression}\n");
        assert!(matches!(
            analyze(&request(&source, expression, None)).unwrap().outcome,
            AnalysisOutcome::Rejected(_)
        ));
    }
}

#[test]
fn unsupported_datetime_factories_are_not_declared() {
    for expression in [
        "datetime.fromtimestamp(0)",
        "datetime.utcfromtimestamp(0)",
        "datetime.utcnow()",
        "date.fromordinal(1)",
        "date.fromisocalendar(2024, 1, 1)",
    ] {
        let source = format!("from datetime import datetime, date\nvalue = {expression}");
        assert!(
            matches!(
                analyze(&request(&source, expression, None)).unwrap().outcome,
                AnalysisOutcome::Rejected(_)
            ),
            "{expression}"
        );
    }
}

#[test]
fn advisory_lints_do_not_reject_valid_expression_types() {
    let expression = "7 if 'yes' else 0";
    let source = format!("value = {expression}");
    let graph = infer(&source, expression, Some("class Extra: ...\n"));
    assert!(!graph.roots.is_empty());
    let invalid = "'yes' - 1";
    assert!(matches!(
        analyze(&request(&format!("value = {invalid}"), invalid, None))
            .unwrap()
            .outcome,
        AnalysisOutcome::Rejected(_)
    ));
}
