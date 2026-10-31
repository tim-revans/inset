//! View options: what a picture shows. The data always holds everything.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use crate::theme::Theme;

/// A kind of label a view can leave off. The names are the option values of
/// `--hide`, and the table in `docs/language/diagrams.md` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hide {
    Docs,
    SettingTypes,
    GrantTypes,
    GrantValues,
    CapabilityNames,
    CapabilityTypes,
    CapabilityOperations,
    Handoffs,
    FieldTypes,
    OutletTypes,
    Signatures,
    MessageParams,
    FailureReasons,
    Failures,
}

impl Hide {
    pub const ALL: [Hide; 14] = [
        Hide::Docs,
        Hide::SettingTypes,
        Hide::GrantTypes,
        Hide::GrantValues,
        Hide::CapabilityNames,
        Hide::CapabilityTypes,
        Hide::CapabilityOperations,
        Hide::Handoffs,
        Hide::FieldTypes,
        Hide::OutletTypes,
        Hide::Signatures,
        Hide::MessageParams,
        Hide::FailureReasons,
        Hide::Failures,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Hide::Docs => "docs",
            Hide::SettingTypes => "setting-types",
            Hide::GrantTypes => "grant-types",
            Hide::GrantValues => "grant-values",
            Hide::CapabilityNames => "capability-names",
            Hide::CapabilityTypes => "capability-types",
            Hide::CapabilityOperations => "capability-operations",
            Hide::Handoffs => "handoffs",
            Hide::FieldTypes => "field-types",
            Hide::OutletTypes => "outlet-types",
            Hide::Signatures => "signatures",
            Hide::MessageParams => "message-params",
            Hide::FailureReasons => "failure-reasons",
            Hide::Failures => "failures",
        }
    }
}

impl fmt::Display for Hide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Hide {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Hide::ALL
            .into_iter()
            .find(|hide| hide.name() == s)
            .ok_or_else(|| {
                let names: Vec<_> = Hide::ALL.iter().map(|h| h.name()).collect();
                format!("unknown label `{s}`; expected one of {}", names.join(", "))
            })
    }
}

/// What one picture shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    /// Draw this function's flowchart, by ID, instead of the component view.
    pub flowchart: Option<String>,
    /// Components to draw with their children inside, by ID. Expanding a
    /// component expands the components around it too.
    pub expand: BTreeSet<String>,
    /// Draw every component with its children inside.
    pub expand_all: bool,
    /// Labels to leave off.
    pub hide: BTreeSet<Hide>,
    /// The palette.
    pub theme: Theme,
    /// Leave the background off, so the picture sits on whatever is behind it.
    pub transparent: bool,
    /// Mark what the requirements reached, from the diagram's `coverage`.
    pub coverage: bool,
}

/// The view options `inset diagram` and `inset-diagram render` take.
pub const VIEW_OPTIONS: &str =
    "  --expand <component id>   draw the component with its children inside (repeatable)
  --expand-all              draw every component with its children inside
  --flowchart <function id> draw the function's flowchart instead
  --hide <label>            leave a kind of label off (repeatable)
  --theme <auto|dark|light> the palette; auto, the default, follows the reader's setting
  --transparent             leave the background off
  --coverage                mark what the requirements reached, from the last
                            `inset test --coverage`
  -o <file>                 write the SVG to a file instead of standard output";

impl View {
    /// Reads the options in [`VIEW_OPTIONS`]: the view, and the file given
    /// with `-o`, if any.
    pub fn from_args(args: &[String]) -> Result<(View, Option<String>), String> {
        let mut view = View::default();
        let mut out = None;
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            let mut value = || rest.next().cloned().ok_or(format!("{arg} needs a value"));
            match arg.as_str() {
                "--expand" => {
                    view.expand.insert(value()?);
                }
                "--expand-all" => view.expand_all = true,
                "--flowchart" => view.flowchart = Some(value()?),
                "--hide" => {
                    view.hide.insert(value()?.parse::<Hide>()?);
                }
                "--theme" => view.theme = value()?.parse()?,
                "--transparent" => view.transparent = true,
                "--coverage" => view.coverage = true,
                "-o" => out = Some(value()?),
                _ => return Err(format!("unknown option `{arg}`")),
            }
        }
        Ok((view, out))
    }

    pub(crate) fn expands(&self, component: &str) -> bool {
        self.expand_all
            || self.expand.iter().any(|id| {
                id == component
                    || id
                        .strip_prefix(component)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
    }

    pub(crate) fn shows(&self, opts: &[Hide]) -> bool {
        !opts.iter().any(|opt| self.hide.contains(opt))
    }
}
