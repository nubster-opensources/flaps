//! Round-trip tests: parsing the serialized form of a parsed document must
//! yield the same model. Serialization is canonical, with `$evaluators`
//! inlined and keys ordered, so byte equality with the source is not a goal.

use flaps_eval::FlagSet;

fn assert_roundtrip(document: &str) {
    let parsed = FlagSet::from_json(document).expect("valid document");
    let serialized = parsed.to_json();
    let reparsed = FlagSet::from_json(&serialized).expect("serialized output parses");

    assert_eq!(parsed, reparsed);
}

#[test]
fn roundtrips_the_upstream_documentation_example() {
    assert_roundtrip(
        r#"{
            "flags": {
                "new-welcome-banner": {
                    "state": "ENABLED",
                    "variants": { "on": true, "off": false },
                    "defaultVariant": "off",
                    "targeting": {
                        "if": [
                            { "ends_with": [{ "var": "email" }, "@example.com"] },
                            "on",
                            "off"
                        ]
                    },
                    "metadata": { "version": "17" }
                }
            },
            "metadata": { "team": "user-experience", "flagSetId": "ecommerce" }
        }"#,
    );
}

#[test]
fn roundtrips_every_operator_family() {
    assert_roundtrip(
        r#"{
            "flags": {
                "kitchen-sink": {
                    "state": "ENABLED",
                    "variants": { "on": true, "off": false },
                    "defaultVariant": "off",
                    "targeting": {
                        "if": [
                            {"and": [
                                {"or": [{"!": [{"var": "a"}]}, {"!!": [{"var": "b"}]}]},
                                {"==": [{"var": ["plan", "free"]}, "pro"]},
                                {"!==": [{"var": "x"}, null]},
                                {"<": [0, {"var": "age"}, 99]},
                                {">=": [{"+": [1, 2]}, {"-": [5, 2]}]},
                                {"in": [{"var": "country"}, ["FR", "BE"]]},
                                {"starts_with": [{"var": "ip"}, "10."]},
                                {"sem_ver": [{"var": "version"}, ">=", "1.2.3"]},
                                {"missing_some": [1, ["email", "phone"]]},
                                {"some": [{"merge": [[1], [2]]}, {">": [{"var": ""}, 0]}]},
                                {"==": [{"reduce": [{"map": [{"filter": [{"var": "xs"}, true]}, {"*": [{"var": ""}, 2]}]}, {"+": [{"var": "current"}, {"var": "accumulator"}]}, 0]}, {"%": [{"max": [10, 20]}, {"min": [3, {"/": [10, 2]}]}]}]},
                                {"in": [{"substr": [{"cat": [{"var": "$flagd.flagKey"}, "-x"]}, 0, 5]}, {"var": "hay"}]},
                                {"!": [{"missing": ["email"]}]}
                            ]},
                            "on",
                            "off"
                        ]
                    }
                },
                "rollout": {
                    "state": "ENABLED",
                    "variants": { "red": "r", "green": "g" },
                    "defaultVariant": "red",
                    "targeting": {
                        "fractional": [
                            {"cat": [{"var": "$flagd.flagKey"}, {"var": "email"}]},
                            ["red", 50],
                            ["green", 50]
                        ]
                    }
                },
                "shorthand-rollout": {
                    "state": "DISABLED",
                    "variants": { "red": 1, "green": 2 },
                    "defaultVariant": "red",
                    "targeting": { "fractional": [["red"], ["green", 3]] }
                }
            }
        }"#,
    );
}

#[test]
fn serialization_inlines_evaluators_and_drops_references() {
    let document = r#"{
        "$evaluators": {
            "internal": {"ends_with": [{"var": "email"}, "@example.com"]}
        },
        "flags": {
            "test-flag": {
                "state": "ENABLED",
                "variants": { "on": true, "off": false },
                "defaultVariant": "off",
                "targeting": {"if": [{"$ref": "internal"}, "on", "off"]}
            }
        }
    }"#;

    let parsed = FlagSet::from_json(document).expect("valid document");
    let serialized = parsed.to_json();

    assert!(!serialized.contains("$ref"));
    assert!(!serialized.contains("$evaluators"));
    assert!(serialized.contains("ends_with"));
    assert_eq!(parsed, FlagSet::from_json(&serialized).expect("reparses"));
}
