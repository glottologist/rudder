// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2019-2020 Normation SAS

use super::Generator;
use crate::ast::enums::EnumExpression;
use crate::ast::resource::*;
use crate::ast::value::*;
use crate::ast::*;
use crate::parser::*;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::error::*;

/*
    DSC parameter types:

    [int]       32-bit signed integer                   => no match
    [long]  	64-bit signed integer                   => no match
    [string] 	Fixed-length string of Unicode chars    => corresponds to our String type
    [char]  	Unicode 16-bit character                => no match
    [bool]  	True/false value                        => corresponds to our Boolean type
    [byte]  	8-bit unsigned integer                  => no match
    [double]  	Double-precision 64-bit fp numbers      => corresponds to our Number type
    [decimal]  	128-bit decimal value                   => no match
    [single]  	Single-precision 32-bit fp numbers      => no match
    [array]  	Array of values                         =>
    [xml]       Xmldocument object                      => no match
    [hashtable] Hashtable object (~Dictionary~)         =>
*/

pub struct DSC {
    // list of already formatted expression in current case
    current_cases: Vec<String>,
    // match enum local variables with class prefixes
    var_prefixes: HashMap<String, String>,
    // already used class prefix
    prefixes: HashMap<String, u32>,
    // condition to add for every other condition for early return
    return_condition: Option<String>,
}

impl DSC {
    pub fn new() -> Self {
        Self {
            current_cases: Vec::new(),
            var_prefixes: HashMap::new(),
            prefixes: HashMap::new(),
            return_condition: None,
        }
    }

    fn new_var(&mut self, prefix: &str) {
        let id = self.prefixes.get(prefix).unwrap_or(&0) + 1;
        self.prefixes.insert(prefix.to_string(), id);
        let var = format!("{}{}", prefix, id);
        self.var_prefixes.insert(prefix.to_string(), var);
    }
    fn reset_cases(&mut self) {
        // TODO this make case in case fail
        self.current_cases = Vec::new();
    }
    fn reset_context(&mut self) {
        self.var_prefixes = HashMap::new();
        self.return_condition = None;
    }

    fn parameter_to_dsc(&self, param: &Value, param_name: &str) -> Result<String> {
        Ok(match param {
            Value::String(s) => {
                // TODO integrate name to parameters
                let param_value = s.format(
                    |x: &str| {
                        x.replace("\\", "\\\\") // backslash escape
                            .replace("\"", "\\\"") // quote escape
                            .replace("$", "${const.dollar}")
                    }, // dollar escape
                    |y: &str| format!("${{{}}}", y), // variable inclusion
                );
                format!(r#"-{} "{}""#, param_name, param_value)
            }
            Value::Number(_, _) => unimplemented!(),
            Value::Boolean(_, _) => unimplemented!(),
            Value::EnumExpression(_e) => "".into(), // TODO
            Value::List(_) => unimplemented!(),
            Value::Struct(_) => unimplemented!(),
        })
    }

    fn format_case_expr(&mut self, gc: &AST, case: &EnumExpression) -> Result<String> {
        Ok(match case {
            EnumExpression::And(e1, e2) => {
                let mut lexpr = self.format_case_expr(gc, e1)?;
                let mut rexpr = self.format_case_expr(gc, e2)?;
                if lexpr.contains("|") {
                    lexpr = format!("({})", lexpr);
                }
                if rexpr.contains("|") {
                    rexpr = format!("({})", rexpr);
                }
                format!("{}.{}", lexpr, rexpr)
            }
            EnumExpression::Or(e1, e2) => format!(
                "{}|{}",
                self.format_case_expr(gc, e1)?,
                self.format_case_expr(gc, e2)?
            ),
            // TODO what about classes that have not yet been set ? can it happen ?
            EnumExpression::Not(e1) => {
                let mut expr = self.format_case_expr(gc, e1)?;
                if expr.contains("|") || expr.contains("&") {
                    expr = format!("!({})", expr);
                }
                format!("!{}", expr)
            }
            EnumExpression::Compare(var, e, item) => {
                if let Some(true) = gc.enum_list.enum_is_global(*e) {
                    // We probably need some translation here since not all enums are available in cfengine (ex debian_only)
                    item.fragment().to_string() // here
                } else {
                    // concat var name + item
                    let prefix = &self.var_prefixes[var.fragment()];
                    // TODO there may still be some conflicts with var or enum containing '_'
                    format!("{}_{}_{}", prefix, e.fragment(), item.fragment())
                }
            }
            EnumExpression::RangeCompare(_var, _e, _item1, _item2) => unimplemented!(), // TODO
            EnumExpression::Default(_) => {
                // extract current cases and build an opposite expression
                if self.current_cases.is_empty() {
                    "any".to_string()
                } else {
                    format!("!({})", self.current_cases.join("|"))
                }
            }
            EnumExpression::NoDefault(_) => "".to_string(),
        })
    }

    fn get_method_parameters(&self, gc: &AST, state_decl: &StateDeclaration) -> Result<String> {
        // depending on whether class_parameters should only be used for generic_methods or not
        // might better handle relative errors as panic! rather than Error::User

        let state_def = match gc.resources.get(&state_decl.resource) {
            Some(r) => match r.states.get(&state_decl.state) {
                Some(s) => s,
                None => panic!(
                    "No method relies on the \"{}\" state for \"{}\"",
                    state_decl.state.fragment(),
                    state_decl.resource.fragment()
                ),
            },
            None => panic!(
                "No method relies on the \"{}\" resource",
                state_decl.resource.fragment()
            ),
        };

        let mut param_names = state_def
            .parameters
            .iter()
            .map(|p| p.name.fragment())
            .collect::<Vec<&str>>();

        let mut class_param_names = match state_def.metadata.get(&Token::from("class_parameters")) {
            Some(Value::Struct(parameters)) => parameters
                .iter()
                .map(|p| {
                    let p_index = match p.1 {
                        Value::Number(_, n) => *n as usize,
                        _ => {
                            return Err(Error::User(String::from(
                                "Expected value type for class parameters metadata: Number",
                            )))
                        }
                    };
                    Ok((p.0.as_ref(), p_index))
                })
                .collect::<Result<Vec<(&str, usize)>>>()?,
            _ => {
                // swap to info! if class_parameter for all methods is a thing
                debug!(
                    "The {}_{} method has no class_parameters metadata attached",
                    state_decl.state.fragment(),
                    state_decl.resource.fragment()
                );
                Vec::new()
            }
        };
        class_param_names.sort_by(|a, b| a.1.cmp(&b.1));

        for (name, index) in class_param_names {
            if index <= param_names.len() {
                param_names.insert(index, name);
            } else {
                return Err(Error::User(String::from(
                    "Class parameter indexes are out of method bounds",
                )));
            }
        }

        // TODO setup mode and output var by calling ... bundle
        map_strings_results(
            state_decl
                .resource_params
                .iter()
                .chain(state_decl.state_params.iter())
                .enumerate(),
            |(i, x)| self.parameter_to_dsc(x, param_names.get(i).unwrap_or(&&"unnamed")),
            " ",
        )
    }

    // TODO simplify expression and remove useless conditions for more readable cfengine
    // TODO underscore escapement
    // TODO how does cfengine use utf8
    // TODO variables
    // TODO comments and metadata
    // TODO use in_class everywhere
    fn format_statement(&mut self, gc: &AST, st: &Statement) -> Result<String> {
        match st {
            Statement::StateDeclaration(sd) => {
                if let Some(var) = sd.outcome {
                    self.new_var(&var);
                }
                let component = match sd.metadata.get(&"component".into()) {
                    // TODO use static_to_string
                    Some(Value::String(s)) => match &s.data[0] {
                        PInterpolatedElement::Static(st) => st.clone(),
                        _ => "any".to_string(),
                    },
                    _ => "any".to_string(),
                };

                Ok(format!(
                    r#"  $local_classes = Merge-ClassContext $local_classes $({} {} -componentName "{}" -reportId $reportId -techniqueName $techniqueName -auditOnly:$auditOnly).get_item("classes")"#,
                    pascebab_case(&component),
                    self.get_method_parameters(gc, sd)?,
                    component,
                ))
            }
            Statement::Case(_case, vec) => {
                self.reset_cases();
                map_strings_results(
                    vec.iter(),
                    |(_case, vst)| {
                        // TODO case in case
                        // let case_exp = self.format_case_expr(gc, case)?;
                        map_strings_results(vst.iter(), |st| self.format_statement(gc, st), "")
                    },
                    "",
                )
            }
            Statement::Fail(msg) => Ok(format!(
                "      \"method_call\" usebundle => ncf_fail({});\n",
                self.parameter_to_dsc(msg, "Fail")?
            )),
            Statement::Log(msg) => Ok(format!(
                "      \"method_call\" usebundle => ncf_log({});\n",
                self.parameter_to_dsc(msg, "Log")?
            )),
            Statement::Return(outcome) => {
                // handle end of bundle
                self.return_condition = Some(match self.current_cases.last() {
                    None => "!any".into(),
                    Some(c) => format!("!({})", c),
                });
                Ok(if *outcome == Token::new("", "kept") {
                    "      \"method_call\" usebundle => success();\n".into()
                } else if *outcome == Token::new("", "repaired") {
                    "      \"method_call\" usebundle => repaired();\n".into()
                } else {
                    "      \"method_call\" usebundle => error();\n".into()
                })
            }
            Statement::Noop => Ok(String::new()),
            // TODO Statement::VariableDefinition()
            _ => Ok(String::new()),
        }
    }

    fn value_to_string(&mut self, value: &Value, string_delim: bool) -> Result<String> {
        let delim = if string_delim { "\"" } else { "" };
        Ok(match value {
            Value::String(s) => format!(
                "{}{}{}",
                delim,
                s.data
                    .iter()
                    .map(|t| match t {
                        PInterpolatedElement::Static(s) => {
                            // replace ${const.xx}
                            s.replace("$", "${consr.dollar}")
                                .replace("\\n", "${const.n}")
                                .replace("\\r", "${const.r}")
                                .replace("\\t", "${const.t}")
                        }
                        PInterpolatedElement::Variable(v) => {
                            // translate variable name
                            format!("${{{}}}", v)
                        }
                    })
                    .collect::<Vec<String>>()
                    .join(""),
                delim
            ),
            Value::Number(_, n) => format!("{}", n),
            Value::Boolean(_, b) => format!("{}", b),
            Value::EnumExpression(_e) => unimplemented!(),
            Value::List(l) => format!(
                "[ {} ]",
                map_strings_results(l.iter(), |x| self.value_to_string(x, true), ",")?
            ),
            Value::Struct(s) => format!(
                "{{ {} }}",
                map_strings_results(
                    s.iter(),
                    |(x, y)| Ok(format!(r#""{}":{}"#, x, self.value_to_string(y, true)?)),
                    ","
                )?
            ),
        })
    }

    fn generate_parameters_metadatas<'src>(&mut self, parameters: Option<Value<'src>>) -> String {
        let mut params_str = String::new();

        let mut get_param_field = |param: &Value, entry: &str| -> String {
            if let Value::Struct(param) = &param {
                if let Some(val) = param.get(entry) {
                    if let Ok(val_s) = self.value_to_string(val, false) {
                        return match val {
                            Value::String(_) => format!("{:?}: {:?}", entry, val_s),
                            _ => format!("{:?}: {}", entry, val_s),
                        };
                    }
                }
            }
            "".to_owned()
        };

        if let Some(Value::List(parameters)) = parameters {
            parameters.iter().for_each(|param| {
                params_str.push_str(&format!(
                    "# @parameter {{ {}, {}, {} }}\n",
                    get_param_field(param, "name"),
                    get_param_field(param, "id"),
                    get_param_field(param, "constraints")
                ));
            });
        };
        params_str
    }

    fn generate_ncf_metadata(&mut self, _name: &Token, resource: &ResourceDef) -> Result<String> {
        let mut meta = resource.metadata.clone();
        // removes parameters from meta and returns it formatted
        let parameters: String =
            self.generate_parameters_metadatas(meta.remove(&Token::from("parameters")));
        // description must be the last field
        let mut map = map_hashmap_results(meta.iter(), |(n, v)| {
            Ok((n.fragment(), self.value_to_string(v, false)?))
        })?;
        let mut metadatas = String::new();
        let mut push_metadata = |entry: &str| {
            if let Some(val) = map.remove(entry) {
                metadatas.push_str(&format!("# @{} {:#?}\n", entry, val));
            }
        };
        push_metadata("name");
        push_metadata("description");
        push_metadata("version");
        metadatas.push_str(&parameters);
        for (key, val) in map.iter() {
            metadatas.push_str(&format!("# @{} {}\n", key, val));
        }
        Ok(metadatas)
    }

    pub fn format_param_type(&self, value: &Value) -> String {
        String::from(match value {
            Value::String(_) => "string",
            Value::Number(_, _) => "long",
            Value::Boolean(_, _) => "bool",
            Value::EnumExpression(_) => "enum_expression",
            Value::List(_) => "list",
            Value::Struct(_) => "struct",
        })
    }
}

impl Generator for DSC {
    // TODO methods differ if this is a technique generation or not
    fn generate(
        &mut self,
        gc: &AST,
        source_file: Option<&Path>,
        dest_file: Option<&Path>,
        _generic_methods: &Path,
        technique_metadata: bool,
    ) -> Result<()> {
        let mut files: HashMap<String, String> = HashMap::new();
        // TODO add global variable definitions
        for (rn, res) in gc.resources.iter() {
            for (sn, state) in res.states.iter() {
                // This condition actually rejects every file that is not the input filename
                // therefore preventing from having an output in another directory
                // Solutions: check filename rather than path, or accept everything that is not from crate root lib
                let file_to_create = match get_dest_file(source_file, sn.file(), dest_file) {
                    Some(file) => file,
                    None => continue,
                };
                self.reset_context();

                // get header
                let header = match files.get(&file_to_create) {
                    Some(s) => s.to_string(),
                    None => {
                        if technique_metadata {
                            self.generate_ncf_metadata(rn, res)? // TODO dsc
                        } else {
                            String::new()
                        }
                    }
                };

                // get parameters
                let method_parameters: String = res
                    .parameters
                    .iter()
                    .chain(state.parameters.iter())
                    .map(|p| {
                        format!(
                            // TODO check if parameter is actually mandatory
                            "    [parameter(Mandatory=$true)]\n    [{}]${}",
                            self.format_param_type(&p.value),
                            pascebab_case(p.name.fragment())
                        )
                    })
                    .collect::<Vec<String>>()
                    .join(",\n");

                // add default dsc parameters
                let parameters: String = vec![
                    String::from("    [parameter(Mandatory=$true)]\n    [string]$techniqueName"),
                    String::from("    [parameter(Mandatory=$true)]\n    [string]$reportId"),
                    String::from("    [switch]$auditOnly"),
                    method_parameters,
                ]
                .join(",\n");

                // get methods
                let methods = &state
                    .statements
                    .iter()
                    .map(|st| self.format_statement(gc, st))
                    .collect::<Result<Vec<String>>>()?
                    .join("\n");
                // merge header + parameters + methods with technique file body
                let content = format!(
                    r#"# generated by rudder-lang
{header}
function {resource_name}-{state_name} {{
  [CmdletBinding()]
  param (
{parameters}
  )
  
  $local_classes = New-ClassContext
  $resources_dir = $PSScriptRoot + "\resources"

{methods}
}}"#,
                    header = header,
                    resource_name = pascebab_case(rn.fragment()),
                    state_name = pascebab_case(sn.fragment()),
                    parameters = parameters,
                    methods = methods
                );
                files.insert(file_to_create, content);
            }
        }

        // create file if needed
        if files.is_empty() {
            match dest_file {
                Some(filename) => File::create(filename).expect("Could not create output file"),
                None => return Err(Error::User("No file to create".to_owned())),
            };
        }

        // write to file
        for (name, content) in files.iter() {
            let mut file = File::create(name).expect("Could not create output file");
            file.write_all(content.as_bytes())
                .expect("Could not write content into output file");
        }
        Ok(())
    }
}

fn pascebab_case(s: &str) -> String {
    let chars = s.chars().into_iter();

    let mut pascebab = String::new();
    let mut is_next_uppercase = true;
    for c in chars {
        let next = match c {
            ' ' | '_' | '-' => {
                is_next_uppercase = true;
                String::from("-")
            }
            c => {
                if is_next_uppercase {
                    is_next_uppercase = false;
                    c.to_uppercase().to_string()
                } else {
                    c.to_string()
                }
            }
        };
        pascebab.push_str(&next);
    }
    pascebab
}

fn get_dest_file(input: Option<&Path>, cur_file: &str, output: Option<&Path>) -> Option<String> {
    let dest_file = match input {
        Some(filepath) => {
            if filepath.file_name() != Some(&OsStr::new(cur_file)) {
                return None;
            }
            // can unwrap here since if source_file is Some, so does dest_file (see end of compile.rs)
            match output.unwrap().to_str() {
                Some(dest_filename) => dest_filename,
                None => cur_file,
            }
        }
        None => cur_file,
    };
    Some(dest_file.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dest_file() {
        assert_eq!(
            get_dest_file(
                Some(Path::new("/path/my_file.rl")),
                "my_file.rl",
                Some(Path::new(""))
            ),
            Some("".to_owned())
        );
        assert_eq!(
            get_dest_file(
                Some(Path::new("/path/my_file.rl")),
                "my_file.rl",
                Some(Path::new("/output/file.rl.dsc"))
            ),
            Some("/output/file.rl.dsc".to_owned())
        );
        assert_eq!(
            get_dest_file(
                Some(Path::new("/path/my_file.rl")),
                "wrong_file.rl",
                Some(Path::new("/output/file.rl.dsc"))
            ),
            None
        );
    }
}
