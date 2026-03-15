mod common;

use arazzo_spec::{
    CriterionExpressionType, CriterionType, ParamLocation, Parameter, RequestBody, Step,
    StepTarget, SuccessCriterion, Workflow,
};
use common::*;
use std::collections::BTreeMap;

fn xml_response(body: &str) -> MockHttpResponse {
    let mut headers = BTreeMap::new();
    headers.insert("Content-Type".to_string(), "text/xml".to_string());
    MockHttpResponse {
        status: 200,
        headers,
        body: body.to_string(),
    }
}

fn soap_step(
    step_id: &str,
    soap_action: &str,
    payload: &str,
    xpath_criterion: &str,
    outputs: Vec<(&str, &str)>,
) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath("POST /soap".to_string())),
        parameters: vec![
            Parameter {
                name: "Content-Type".to_string(),
                in_: Some(ParamLocation::Header),
                value: serde_yaml_ng::Value::String("text/xml".to_string()),
                ..Parameter::default()
            },
            Parameter {
                name: "SOAPAction".to_string(),
                in_: Some(ParamLocation::Header),
                value: serde_yaml_ng::Value::String(soap_action.to_string()),
                ..Parameter::default()
            },
        ],
        request_body: Some(RequestBody {
            content_type: "text/xml".to_string(),
            payload: Some(serde_yaml_ng::Value::String(payload.to_string())),
            ..RequestBody::default()
        }),
        success_criteria: vec![
            SuccessCriterion {
                condition: "$statusCode == 200".to_string(),
                ..SuccessCriterion::default()
            },
            SuccessCriterion {
                condition: xpath_criterion.to_string(),
                type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                    type_: "xpath".to_string(),
                    version: String::new(),
                })),
                ..SuccessCriterion::default()
            },
        ],
        outputs: outputs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Step::default()
    }
}

// ── SOAP round-trip: XML body → mock server → XPath extraction ──

#[tokio::test]
async fn soap_xml_body_sent_as_raw_bytes() {
    let server = start_server(|_method, _url, headers, body| {
        // Verify the body is raw XML, NOT JSON-quoted
        assert!(
            body.contains("<?xml"),
            "body should be raw XML, got: {body}"
        );
        assert!(
            !body.starts_with('"'),
            "body should NOT be JSON-quoted, got: {body}"
        );

        let action = header_value(&headers, "SOAPAction").unwrap_or_default();
        if action.contains("GetCustomer") {
            xml_response(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <GetCustomerResponse>
      <Customer>
        <Id>42</Id>
        <Name>Alice</Name>
      </Customer>
    </GetCustomerResponse>
  </soap:Body>
</soap:Envelope>"#,
            )
        } else {
            MockHttpResponse::empty(404)
        }
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "soap-test".to_string(),
        steps: vec![soap_step(
            "get-customer",
            "GetCustomer",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <GetCustomer><CustomerId>42</CustomerId></GetCustomer>
  </soap:Body>
</soap:Envelope>"#,
            "//GetCustomerResponse/Customer/Id",
            vec![
                ("customer_id", "//GetCustomerResponse/Customer/Id"),
                ("customer_name", "//GetCustomerResponse/Customer/Name"),
            ],
        )],
        outputs: BTreeMap::from([
            (
                "customer_id".to_string(),
                "$steps.get-customer.outputs.customer_id".to_string(),
            ),
            (
                "customer_name".to_string(),
                "$steps.get-customer.outputs.customer_name".to_string(),
            ),
        ]),
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute_collect("soap-test", BTreeMap::new()).await;

    let outputs = match result.outputs {
        Ok(o) => o,
        Err(err) => panic!("expected success, got: {err}"),
    };
    assert_eq!(
        outputs.get("customer_id").and_then(|v| v.as_str()),
        Some("42")
    );
    assert_eq!(
        outputs.get("customer_name").and_then(|v| v.as_str()),
        Some("Alice")
    );
}

#[tokio::test]
async fn soap_multi_step_with_interpolation() {
    let server = start_server(|_method, _url, headers, body| {
        let action = header_value(&headers, "SOAPAction").unwrap_or_default();
        if action.contains("ListCustomers") {
            xml_response(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <ListCustomersResponse>
      <TotalCount>3</TotalCount>
      <Customers>
        <Customer><Id>101</Id><Name>Bob</Name></Customer>
      </Customers>
    </ListCustomersResponse>
  </soap:Body>
</soap:Envelope>"#,
            )
        } else if action.contains("GetCustomer") {
            // Verify the interpolated ID made it into the body
            assert!(
                body.contains("<CustomerId>101</CustomerId>"),
                "interpolated customer ID should be 101, got body: {body}"
            );
            xml_response(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <GetCustomerResponse>
      <Customer>
        <Id>101</Id>
        <Name>Bob</Name>
        <Email>bob@example.com</Email>
      </Customer>
    </GetCustomerResponse>
  </soap:Body>
</soap:Envelope>"#,
            )
        } else {
            MockHttpResponse::empty(404)
        }
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "soap-chain".to_string(),
        steps: vec![
            soap_step(
                "list",
                "ListCustomers",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body><ListCustomers/></soap:Body>
</soap:Envelope>"#,
                "//ListCustomersResponse/TotalCount",
                vec![
                    ("total", "//ListCustomersResponse/TotalCount"),
                    (
                        "first_id",
                        "//ListCustomersResponse/Customers/Customer[1]/Id",
                    ),
                ],
            ),
            soap_step(
                "get",
                "GetCustomer",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <GetCustomer>
      <CustomerId>{$steps.list.outputs.first_id}</CustomerId>
    </GetCustomer>
  </soap:Body>
</soap:Envelope>"#,
                "//GetCustomerResponse/Customer/Id",
                vec![
                    ("name", "//GetCustomerResponse/Customer/Name"),
                    ("email", "//GetCustomerResponse/Customer/Email"),
                ],
            ),
        ],
        outputs: BTreeMap::from([
            ("total".to_string(), "$steps.list.outputs.total".to_string()),
            (
                "first_id".to_string(),
                "$steps.list.outputs.first_id".to_string(),
            ),
            ("name".to_string(), "$steps.get.outputs.name".to_string()),
            ("email".to_string(), "$steps.get.outputs.email".to_string()),
        ]),
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute_collect("soap-chain", BTreeMap::new()).await;

    let outputs = match result.outputs {
        Ok(o) => o,
        Err(err) => panic!("expected success, got: {err}"),
    };
    assert_eq!(outputs.get("total").and_then(|v| v.as_str()), Some("3"));
    assert_eq!(
        outputs.get("first_id").and_then(|v| v.as_str()),
        Some("101")
    );
    assert_eq!(outputs.get("name").and_then(|v| v.as_str()), Some("Bob"));
    assert_eq!(
        outputs.get("email").and_then(|v| v.as_str()),
        Some("bob@example.com")
    );
}

#[tokio::test]
async fn soap_xpath_criterion_failure() {
    let server = start_server(|_method, _url, _headers, _body| {
        xml_response(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <Fault><faultstring>Not found</faultstring></Fault>
  </soap:Body>
</soap:Envelope>"#,
        )
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "soap-fail".to_string(),
        steps: vec![soap_step(
            "get",
            "GetCustomer",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body><GetCustomer><CustomerId>999</CustomerId></GetCustomer></soap:Body>
</soap:Envelope>"#,
            // This XPath won't match the Fault response
            "//GetCustomerResponse/Customer/Id",
            vec![],
        )],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute_collect("soap-fail", BTreeMap::new()).await;

    assert!(
        result.outputs.is_err(),
        "workflow should fail on XPath mismatch"
    );
}
