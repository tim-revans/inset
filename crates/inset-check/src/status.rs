//! HTTP statuses by the names requirement steps and `#[status]` use.

/// Each status's name, in `UpperCamelCase`, with its code. The names are
/// those of the HTTP specification (RFC 9110), with `OK` as it is written there.
pub const STATUSES: [(&str, u16); 34] = [
    ("Continue", 100),
    ("SwitchingProtocols", 101),
    ("OK", 200),
    ("Created", 201),
    ("Accepted", 202),
    ("NonAuthoritativeInformation", 203),
    ("NoContent", 204),
    ("ResetContent", 205),
    ("PartialContent", 206),
    ("MultipleChoices", 300),
    ("MovedPermanently", 301),
    ("Found", 302),
    ("SeeOther", 303),
    ("NotModified", 304),
    ("TemporaryRedirect", 307),
    ("PermanentRedirect", 308),
    ("BadRequest", 400),
    ("Unauthorized", 401),
    ("Forbidden", 403),
    ("NotFound", 404),
    ("MethodNotAllowed", 405),
    ("NotAcceptable", 406),
    ("Conflict", 409),
    ("Gone", 410),
    ("PreconditionFailed", 412),
    ("ContentTooLarge", 413),
    ("UnsupportedMediaType", 415),
    ("UnprocessableContent", 422),
    ("TooManyRequests", 429),
    ("InternalServerError", 500),
    ("NotImplemented", 501),
    ("BadGateway", 502),
    ("ServiceUnavailable", 503),
    ("GatewayTimeout", 504),
];

/// The code of the status with this name.
pub fn code(name: &str) -> Option<u16> {
    STATUSES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// The status whose name differs from `name` only in case, as a suggestion.
pub(crate) fn by_other_case(name: &str) -> Option<&'static str> {
    STATUSES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(n, _)| *n)
}
