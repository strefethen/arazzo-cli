Yes. There is other compatibility baggage, much of it introduced by this repository. These are the clearest examples I verified in current source:
|Area|	What we still carry|
|-|-|
|Custom operation routing|	Forms such as `GET {petstore}./pets`, plus interpreting a source-description URL as an API base URL. These require our own routing rules. [Implementation (line 744)](/Users/stevetrefethen/github/arazzo-cli/crates/arazzo-runtime/src/runtime_core/engine_http.rs:744)|
|GJSON-style runtime expressions|	Expressions such as `$response.body.users.#(id==2).name`, array-length syntax, and filter variants. They keep handwritten traversal and predicate parsing alive independently of typed JSONPath. [Parser (line 1468)](/Users/stevetrefethen/github/arazzo-cli/crates/arazzo-expr/src/lib.rs:1468)|
|Extra simple-condition operators|	`contains`, `matches`, and `in` have dedicated implementation, although they are absent from Arazzo’s [operator list (line 1242)](/Users/stevetrefethen/github/arazzo-cli/spec/arazzo/v1.1.0.html:1242). [Implementation (line 775)](/Users/stevetrefethen/github/arazzo-cli/crates/arazzo-expr/src/lib.rs:775)
|Legacy action references|	`name: $components.failureActions.foo` triggers component resolution and field-by-field merging. It includes special behavior such as `retryAfter: 0` meaning “keep the inherited value.” [Implementation (line 3738)](/Users/stevetrefethen/github/arazzo-cli/crates/arazzo-validate/src/lib.rs:3738)|
|Bare XPath outputs|	An output string beginning with `/` enters a separate legacy path that implicitly selects XPath 1.0. [Implementation (line 158)](/Users/stevetrefethen/github/arazzo-cli/crates/arazzo-runtime/src/runtime_core/criteria.rs:158)|


**The routing issue is the most consequential**: the [specification’s `operationPath` form (line 677)](/Users/stevetrefethen/github/arazzo-cli/spec/arazzo/v1.1.0.html:677) is still explicitly rejected by the runtime, while our invented forms work. That affects basic workflow portability.

**The GJSON extension matters directly to your parser concern**: replacing both typed JSONPath implementations will still leave this separate handwritten expression machinery. Its syntax goes beyond the [runtime-expression grammar (line 1811)](/Users/stevetrefethen/github/arazzo-cli/spec/arazzo/v1.1.0.html:1811).

There is also a policy contradiction: [AGENTS.md (line 35)](/Users/stevetrefethen/github/arazzo-cli/AGENTS.md:35) says “Approved extensions: none,” while [README.md (line 424)](/Users/stevetrefethen/github/arazzo-cli/README.md:424) says “extensions are permitted.” We have not reconciled the implementation and documentation with the current policy.

XPath’s version list is a separate case: those are published standards; our runtime explicitly supports only 1.0. I found no second comparable expired-draft dependency in the execution paths checked.