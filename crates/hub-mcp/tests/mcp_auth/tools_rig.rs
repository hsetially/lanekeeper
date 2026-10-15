//! A signed-in MCP client and a way to call tools and read what comes back, shared by the tests of the tools (T2).
//!
//! `Called::ok` and `Called::err` check the result against the schema of the contract with a validator that is not ours, so every
//! tool test is also a test that the result is what the document says.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::LazyLock;

use domain::{Role, UserStatus};
use hub_mcp::schemas::Contract;
use jsonschema::Validator;
use serde_json::{Value, json};

use crate::rig::Rig;
use crate::support::Client;

static CONTRACT: LazyLock<Contract> = LazyLock::new(|| Contract::embedded().unwrap());

static RESULT_VALIDATORS: LazyLock<HashMap<String, Validator>> = LazyLock::new(|| {
    CONTRACT
        .tools()
        .iter()
        .map(|t| {
            let schema = Value::Object((**t.result()).clone());
            (
                t.name().to_owned(),
                jsonschema::validator_for(&schema).expect("a result schema compiles"),
            )
        })
        .collect()
});

static ERROR_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    jsonschema::validator_for(&Value::Object((**CONTRACT.error_result()).clone()))
        .expect("the error schema compiles")
});

pub fn contract() -> &'static Contract {
    &CONTRACT
}

/// Register `key` as an active user with `role`, with a token the scripted verifier accepts, and open a session that declares
/// no client capabilities.
pub async fn signed_in(rig: &Rig, key: &str, role: Role) -> Client {
    signed_in_with(rig, key, role, &json!({})).await
}

pub async fn signed_in_with(rig: &Rig, key: &str, role: Role, capabilities: &Value) -> Client {
    rig.add_user(key, Some(role), UserStatus::Active);
    let token = format!("tok-{key}");
    rig.verifier.accept(&token, key);
    let mut client = rig.client(Some(&token));
    client.initialize(capabilities).await.expect("initialize");
    client
}

/// What a `tools/call` answered.
#[derive(Debug, Clone)]
pub struct Called {
    pub tool: String,
    pub response: Value,
}

pub async fn call(client: &mut Client, tool: &str, args: &Value) -> Called {
    let response = client.call_tool_to_end(tool, args).await;
    Called {
        tool: tool.to_owned(),
        response,
    }
}

impl Called {
    pub fn result(&self) -> &Value {
        &self.response["result"]
    }

    pub fn is_error(&self) -> bool {
        self.result()["isError"] == json!(true)
    }

    pub fn structured(&self) -> &Value {
        &self.result()["structuredContent"]
    }

    /// The text of the first `content` block.
    pub fn text(&self) -> String {
        self.result()["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    /// The bytes the cap counts: `structuredContent` and `content`.
    pub fn bytes(&self) -> usize {
        serde_json::to_vec(self.structured()).unwrap().len()
            + serde_json::to_vec(&self.result()["content"]).unwrap().len()
    }

    /// A successful result, valid against the tool's result schema. Returns `data`.
    pub fn ok(&self) -> &Value {
        assert!(
            self.response.get("error").is_none(),
            "{}: a protocol error: {}",
            self.tool,
            self.response
        );
        assert!(
            !self.is_error(),
            "{}: an error result: {}",
            self.tool,
            self.structured()
        );
        let validator = RESULT_VALIDATORS
            .get(&self.tool)
            .unwrap_or_else(|| panic!("no schema for {}", self.tool));
        let errors: Vec<String> = validator
            .iter_errors(self.structured())
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "{}: the result does not match its schema: {errors:?}\n{}",
            self.tool,
            self.structured()
        );
        &self.structured()["data"]
    }

    /// The envelope of a successful result.
    pub fn envelope(&self) -> &Value {
        self.ok();
        self.structured()
    }

    /// A failed call, valid against the error schema. Returns the error's code.
    pub fn err(&self) -> String {
        assert!(
            self.response.get("error").is_none(),
            "{}: a protocol error: {}",
            self.tool,
            self.response
        );
        assert!(
            self.is_error(),
            "{}: not an error: {}",
            self.tool,
            self.structured()
        );
        let errors: Vec<String> = ERROR_VALIDATOR
            .iter_errors(self.structured())
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "{}: the error does not match its schema: {errors:?}\n{}",
            self.tool,
            self.structured()
        );
        self.structured()["error"]["code"].as_str().unwrap().to_owned()
    }

    pub fn error(&self) -> &Value {
        self.err();
        &self.structured()["error"]
    }
}

/// What is wrong with `value` as the `structuredContent` of a failed call, by the contract's error schema.
pub fn error_schema_problems(value: &Value) -> Vec<String> {
    ERROR_VALIDATOR
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect()
}
