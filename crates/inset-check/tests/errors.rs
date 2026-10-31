//! Pins the semantic diagnostics: their wording, and the span each one
//! points at. The cases are in `cases/`, which the language server's tests
//! share.

mod cases;

use cases::{
    COUNTING, COUNTING_REQ, HELLO, HELLO_REQ, SHELF, SHELF_LIBRARY, SHELF_REQ, STORE, STORE_REQ,
    edit,
};
use inset_check::{Program, SourceFile, check};

/// Checks the program made of these files and renders its diagnostics.
fn rendered_files(files: &[(&str, &str)]) -> String {
    let program = Program::from_files(
        files
            .iter()
            .map(|(path, text)| SourceFile {
                path: path.to_string(),
                text: text.to_string(),
            })
            .collect(),
    );
    check(&program)
        .iter()
        .map(|r| program.render(r, false))
        .collect()
}

/// Checks a program of one code file and renders its diagnostics.
fn rendered(main: &str, requirements: &str) -> String {
    rendered_files(&[
        ("src/main.inset", main),
        ("requirements/main.req.inset", requirements),
    ])
}

/// Snapshots the diagnostics of a case, which must not be empty.
fn errors(name: &str) {
    let case = cases::all()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no case `{name}`"));
    let rendered = rendered_files(&case.files());
    assert!(!rendered.is_empty(), "{name}: expected diagnostics");
    insta::with_settings!({ omit_expression => true }, {
        insta::assert_snapshot!(name, rendered);
    });
}

#[test]
fn bases_check_clean() {
    assert_eq!(rendered(HELLO, HELLO_REQ), "");
    assert_eq!(rendered(COUNTING, COUNTING_REQ), "");
    // A value moved into a message, and a closure that only computes.
    let main = edit(
        COUNTING,
        "        send self.log.record(n);",
        "        let text = n.to_string();\n        send self.tally.note(move text);\n        \
         let doubled: Vec<u32> = vec![n].iter().map(move |x| x * 2).collect();\n        \
         send self.log.record(doubled[0]);",
    );
    assert_eq!(rendered(&main, COUNTING_REQ), "");
    assert_eq!(
        rendered_files(&[
            ("src/main.inset", SHELF),
            ("src/library.inset", SHELF_LIBRARY),
            ("requirements/main.req.inset", SHELF_REQ),
        ]),
        ""
    );
    assert_eq!(rendered(STORE, STORE_REQ), "");
}

// ---- The cases Stage 6 of the plan names ------------------------------------

#[test]
fn undeclared_capability() {
    errors("undeclared_capability");
}

#[test]
fn mutable_borrow_across_boundary() {
    errors("mutable_borrow_across_boundary");
}

#[test]
fn unknown_route_in_step() {
    errors("unknown_route_in_step");
}

#[test]
fn unknown_capability_in_step() {
    errors("unknown_capability_in_step");
}

#[test]
fn duplicate_requirement() {
    errors("duplicate_requirement");
}

#[test]
fn failure_arm_without_reason() {
    errors("failure_arm_without_reason");
}

#[test]
fn closure_that_awaits() {
    errors("closure_that_awaits");
}

#[test]
fn unconnected_outlet() {
    errors("unconnected_outlet");
}

#[test]
fn outlet_connected_to_wrong_type() {
    errors("outlet_connected_to_wrong_type");
}

// ---- Other checks ------------------------------------------------------------

#[test]
fn value_across_boundary_without_move() {
    errors("value_across_boundary_without_move");
}

#[test]
fn message_neither_awaited_nor_sent() {
    errors("message_neither_awaited_nor_sent");
}

#[test]
fn send_with_reply() {
    errors("send_with_reply");
}

#[test]
fn capability_lent() {
    errors("capability_lent");
}

#[test]
fn handed_down_field_kept() {
    errors("handed_down_field_kept");
}

#[test]
fn component_starts_itself() {
    errors("component_starts_itself");
}

#[test]
fn route_parameter_unsupplied() {
    errors("route_parameter_unsupplied");
}

#[test]
fn format_names_no_variable() {
    errors("format_names_no_variable");
}

#[test]
fn reply_type_mismatch() {
    errors("reply_type_mismatch");
}

#[test]
fn printing_and_std_io() {
    errors("printing_and_std_io");
}

#[test]
fn setting_and_grant_types() {
    errors("setting_and_grant_types");
}

#[test]
fn step_path_of_wrong_type() {
    errors("step_path_of_wrong_type");
}

#[test]
fn statuses() {
    errors("statuses");
}

#[test]
fn self_without_receiver() {
    errors("self_without_receiver");
}

// ---- Types and modules ---------------------------------------------------------

#[test]
fn variant_without_status() {
    errors("variant_without_status");
}

#[test]
fn error_variant_holding_a_number() {
    errors("error_variant_holding_a_number");
}

#[test]
fn unknown_status_name() {
    errors("unknown_status_name");
}

#[test]
fn private_item_from_another_module() {
    errors("private_item_from_another_module");
}

#[test]
fn private_field_from_another_module() {
    errors("private_field_from_another_module");
}

#[test]
fn module_without_a_file() {
    errors("module_without_a_file");
}

#[test]
fn file_not_a_module() {
    errors("file_not_a_module");
}

#[test]
fn derive_not_allowed() {
    errors("derive_not_allowed");
}

#[test]
fn match_leaves_out_a_case() {
    errors("match_leaves_out_a_case");
}

// ---- Requirement facts and checks --------------------------------------------

#[test]
fn fact_naming_unknown_grant() {
    errors("fact_naming_unknown_grant");
}

#[test]
fn fact_of_wrong_type() {
    errors("fact_of_wrong_type");
}

#[test]
fn fails_after_then() {
    errors("fails_after_then");
}

#[test]
fn check_of_wrong_type() {
    errors("check_of_wrong_type");
}

#[test]
fn compared_without_debug() {
    errors("compared_without_debug");
}

#[test]
fn table_key_not_orderable() {
    errors("table_key_not_orderable");
}
