#![allow(dead_code)]

use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub async fn mock_op(server: &MockServer, op_name: &str, response_json: Value) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({"operationName": op_name})))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_json))
        .mount(server)
        .await;
}

pub async fn mock_op_status(server: &MockServer, op_name: &str, status: u16) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({"operationName": op_name})))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

pub fn library(id: &str, facet: &str, name: &str, roots: &[&str]) -> Value {
    json!({
        "id": id,
        "facet": facet,
        "name": name,
        "roots": roots.iter().enumerate()
            .map(|(i, p)| json!({"path": p, "isDefault": i == 0}))
            .collect::<Vec<_>>(),
        "qualityProfileId": "",
        "requestQualityProfileIds": [],
    })
}

pub fn libraries_response(libs: Vec<Value>) -> Value {
    json!({"data": {"libraries": libs}})
}
