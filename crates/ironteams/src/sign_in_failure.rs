//! What the sign-in screen says when sign-in fails. A refusal by the organization's policy gets a
//! plain message and a report the user can copy; anything else keeps the generic text.

use teams_proto::Error;

/// The text for the error screen.
#[derive(Debug, PartialEq, Eq)]
pub struct Shown {
    pub message: String,
    /// What the "Copy error details" button copies: the AADSTS code, Microsoft's name for it and
    /// a short description. Empty when the failure is not a policy block. It holds no tenant,
    /// user or trace id, so it is safe to paste into a report (the app sends no telemetry, this
    /// is how a block gets reported).
    pub report: String,
}

/// The message and report for a failed sign-in.
pub fn describe(err: &anyhow::Error) -> Shown {
    if let Some(Error::PolicyBlocked {
        code,
        name,
        summary,
        ..
    }) = err.downcast_ref::<Error>()
    {
        return Shown {
            message: "Your organization blocks this sign-in method. IronTeams signs in with a \
                      device code, and a policy of your organization (Conditional Access or a \
                      security setting) refuses it. Ask your IT administrator, and copy the \
                      details below for them."
                .into(),
            report: format!("IronTeams sign-in blocked: AADSTS{code} ({name}). {summary}"),
        };
    }
    Shown {
        message: format!("Sign-in failed: {err}"),
        report: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked() -> anyhow::Error {
        teams_proto::auth::parse_device_code(
            r#"{"error":"invalid_request","error_description":"AADSTS53003: Access has been blocked by Conditional Access policies.\r\nTrace ID: 11111111-2222-3333-4444-555555555555\r\nCorrelation ID: 66666666-7777-8888-9999-000000000000","error_codes":[53003]}"#,
        )
        .unwrap_err()
        .into()
    }

    #[test]
    fn a_policy_block_says_so_and_offers_a_report() {
        let s = describe(&blocked());
        assert!(
            s.message
                .starts_with("Your organization blocks this sign-in method")
        );
        assert_eq!(
            s.report,
            "IronTeams sign-in blocked: AADSTS53003 (BlockedByConditionalAccess). Access has been blocked by Conditional Access policies."
        );
        // No trace or correlation id, tenant or user in what the user pastes.
        assert!(!s.report.contains("1111"));
        assert!(!s.message.contains("1111"));
    }

    #[test]
    fn other_failures_keep_the_generic_message_and_have_no_report() {
        let e: anyhow::Error = teams_proto::auth::parse_token_response(
            r#"{"error":"invalid_grant","error_description":"AADSTS70018: wrong user code"}"#,
        )
        .unwrap_err()
        .into();
        let s = describe(&e);
        assert_eq!(
            s.message,
            "Sign-in failed: auth: invalid_grant: AADSTS70018: wrong user code"
        );
        assert!(s.report.is_empty());
    }
}
