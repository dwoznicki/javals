use std::collections::HashMap;
use std::fs::File;

use log::{info, error};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use dashmap::DashMap;
use tree_sitter::{Parser, Tree, Node, Query, Point};

#[derive(Debug)]
enum TokenType {
    ClassName,
    MemberVariable,
    MethodName(Vec<String>), // parameter types
    ParameterName(Option<String>), // type
    LocalVariable(&TokenLocation), // type location
}

#[derive(Debug)]
struct TokenLocation {
    uri: String,
    start_position: Point,
    end_position: Point,
    token_type: TokenType,
    scope_id: usize,
}

#[derive(Debug)]
struct Backend {
    client: Client,
    // ast_map: DashMap<String, HashMap<String, ()>>,
    document_map: DashMap<String, String>,
    parsed_document_map: DashMap<String, Tree>,
    token_location_map: DashMap<String, Vec<TokenLocation>>,
    // semantic_token_map: DashMap<String, Vec<()>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                // position_encoding: (),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                // selection_range_provider: (),
                // hover_provider: (),
                // completion_provider: (),
                // signature_help_provider: (),
                definition_provider: Some(OneOf::Left(true)),
                // type_definition_provider: (),
                // implementation_provider: (),
                // references_provider: (),
                // document_highlight_provider: (),
                // document_symbol_provider: (),
                // workspace_symbol_provider: (),
                // code_action_provider: (),
                // code_lens_provider: (),
                // document_formatting_provider: (),
                // document_range_formatting_provider: (),
                // document_on_type_formatting_provider: (),
                // rename_provider: (),
                // document_link_provider: (),
                // color_provider: (),
                // folding_range_provider: (),
                // declaration_provider: (),
                // execute_command_provider: (),
                // workspace: (),
                // call_hierarchy_provider: (),
                // semantic_tokens_provider: (),
                // moniker_provider: (),
                // linked_editing_range_provider: (),
                // inline_value_provider: (),
                // inlay_hint_provider: (),
                // diagnostic_provider: (),
                // experimental: (),
                ..ServerCapabilities::default()
            }
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        info!("initialized");
        self.client
            .log_message(MessageType::INFO, "server initialized")
            .await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        info!("did_open");
        self.client
            .log_message(MessageType::INFO, "file opened")
            .await;
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: params.text_document.text,
            version: params.text_document.version,
        })
            .await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        info!("did_change");
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: std::mem::take(&mut params.content_changes[0].text),
            version: params.text_document.version,
        })
            .await;
    }

    async fn did_save(&self, _: DidSaveTextDocumentParams) {
        info!("did_save");
        self.client
            .log_message(MessageType::INFO, "file saved")
            .await;
    }

    async fn did_close(&self, _: DidCloseTextDocumentParams) {
        info!("did_close");
        self.client
            .log_message(MessageType::INFO, "file closed")
            .await;
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        info!("goto_definition {} {:?}", uri.to_string(), position);
        let tree = self.parsed_document_map.get(uri.as_str()).unwrap();
        let source_text = self.document_map.get(uri.as_str()).unwrap();
        let base_node = tree.root_node().named_descendant_for_point_range(
            to_point(position),
            to_point(position),
        )
            .expect(format!("Unable to find node at postion: {:?}", position).as_str());
        if base_node.kind() != "identifier" {
            return Ok(None);
        }
        let token = base_node.utf8_text(source_text.as_bytes()).unwrap();
        info!("found node = {:?}, {:?}", base_node, token);
        let locations = self.token_location_map.get(token);
        if locations.is_none() {
            return Ok(None);
        }
        // 
        {
            let parent_node = base_node.parent().unwrap();
            match parent_node.kind() {
                "field_access" => {
                    let mut cursor = parent_node.walk();
                    let identifier_nodes = parent_node.children(&mut cursor);
                    for identifier_node in identifier_nodes {
                        let identifier_token = identifier_node.utf8_text(source_text.as_bytes()).unwrap();
                    }
                }
                _ => {}
            };
        }
        let map = locations.unwrap().iter().fold(HashMap::new(), |mut map, loc| {
            map.insert(loc.scope_id, (loc.start_position, loc.end_position));
            return map;
        });
        let mut current_node = base_node;
        loop {
            let parent_node = match current_node.parent() {
                Some(node) => node,
                None => break,
            };
            match map.get(&parent_node.id()) {
                Some((start_point, end_point)) => {
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri,
                        range: Range {
                            start: to_position(*start_point),
                            end: to_position(*end_point),
                        },
                    })));
                }
                None => {
                    current_node = parent_node;
                }
            };
        }
        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

struct TextDocumentItem {
    uri: Url,
    text: String,
    version: i32,
}
impl Backend {
    async fn on_change(&self, params: TextDocumentItem) {
        let mut parser = Parser::new();
        parser.set_language(tree_sitter_java::language()).expect("Error loading Java grammar.");

        let tree = match self.parsed_document_map.get(params.uri.as_str()) {
            Some(r) => parser.parse(params.text.as_bytes(), Some(r.value())),
            None => parser.parse(params.text.as_bytes(), None),
        }.expect("Unable to walk tree");
        let nodes: Vec<Node<'_>> = tree_sitter_traversal::traverse(tree.walk(), tree_sitter_traversal::Order::Pre).collect::<Vec<_>>();
        for node in nodes {
            info!("node = {}, {}, {}, {}, {}", node.id(), node.kind(), node.utf8_text(params.text.as_bytes()).unwrap(), node.start_position(), node.end_position());

            if node.kind() != "identifier" {
                continue;
            }

            let parent = node.parent().unwrap();
            let token = node.utf8_text(params.text.as_bytes()).unwrap();
            let (token_type, scope_id) = match parent.kind() {
                "class_declaration" => {
                    (TokenType::ClassName, parent.id())
                }
                "variable_declarator" => {
                    let field_declaration_node = parent.parent().unwrap();
                    match field_declaration_node.kind() {
                        "field_declaration" => {
                            let class_body_node = field_declaration_node.parent().unwrap();
                            if class_body_node.kind() != "class_body" {
                                panic!("expected class_body node, but got {}", class_body_node.kind());
                            }
                            (TokenType::MemberVariable, class_body_node.id())
                        }
                        "local_variable_declaration" => {
                        }
                        _ => {
                            info!("unhandled variable_declarator branch {}", field_declaration_node.kind());
                            continue;
                        }
                    }
                }
                "method_declaration" => {
                    let mut parameter_types: Vec<String> = Vec::new();
                    let params_node = node.next_named_sibling().unwrap();
                    if params_node.kind() == "formal_parameters" {
                        for param_node in params_node.named_children(&mut params_node.walk()) {
                            if param_node.kind() != "formal_parameter" {
                                continue;
                            }
                            for param_child_node in param_node.named_children(&mut param_node.walk()) {
                                match param_child_node.kind() {
                                    "integral_type" | "type_identifier" => {
                                        let parameter_type_token = param_child_node.utf8_text(params.text.as_bytes()).unwrap();
                                        parameter_types.push(parameter_type_token.to_string());
                                    }
                                    _ => continue
                                };
                            }
                        }
                    }
                    (TokenType::MethodName(parameter_types), parent.id())
                }
                "formal_parameter" => {
                    let parameter_type: Option<String> = parent.named_children(&mut parent.walk())
                        .into_iter()
                        .find_map(|n| {
                            match n.kind() {
                                "integral_type" | "type_identifier" => {
                                    Some(n.utf8_text(params.text.as_bytes()).unwrap().to_string())
                                }
                                _ => None
                            }
                        });
                    let method_declaration_node = parent
                        .parent() // formal_parameters
                        .unwrap()
                        .parent() // method_declaration
                        .unwrap();
                    if method_declaration_node.kind() != "method_declaration" {
                        panic!("expected method_declaration node, but got {}", method_declaration_node.kind());
                    }
                    (TokenType::ParameterName(parameter_type), method_declaration_node.id())
                },
                _ => {
                    info!("unhandled branch {}", parent.kind());
                    continue;
                }
            };
            let location = TokenLocation {
                uri: params.uri.to_string(),
                start_position: node.start_position(),
                end_position: node.end_position(),
                token_type,
                scope_id,
            };
            if !self.token_location_map.contains_key(token) {
                self.token_location_map.insert(token.to_string(), Vec::new());
            }
            self.token_location_map.get_mut(token).unwrap().push(location);
        }
        self.document_map.insert(params.uri.to_string(), params.text);
        self.parsed_document_map.insert(params.uri.to_string(), tree);
        info!("map {:#?}", self.token_location_map);
    }
}

// fn main() {
//     let code = r#"
//     class Test {
//         int double(int x) {
//             return x * 2;
//         }
//     }
//     "#;
//     let mut parser = tree_sitter::Parser::new();
//     parser.set_language(tree_sitter_java::language()).expect("Error loading Java grammar.");
//     let tree = parser.parse(code, None).unwrap();
//     let nodes = tree_sitter_traversal::traverse(tree.walk(), tree_sitter_traversal::Order::Pre).collect::<Vec<_>>();
//     for node in nodes {
//         println!("node = {:?}", node);
//     }
// }

fn to_position(point: Point) -> Position {
    return Position {
        line: point.row as u32,
        character: point.column as u32,
    };
}

fn to_point(position: Position) -> Point {
    return Point {
        row: position.line as usize,
        column: position.character as usize,
    };
}

#[tokio::main]
async fn main() {
    let log_file = Box::new(File::create("log.txt").unwrap());
    env_logger::Builder::new()
        .filter(None, log::LevelFilter::Info)
        .target(env_logger::Target::Pipe(log_file))
        .init();
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        document_map: DashMap::new(),
        parsed_document_map: DashMap::new(),
        token_location_map: DashMap::new(),
        // semantic_token_map: DashMap::new(),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

