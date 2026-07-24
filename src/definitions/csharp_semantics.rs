use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CSharpStringId(pub u32);

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CSharpRefKind {
    #[default]
    None,
    Ref,
    Out,
    In,
    RefReadonly,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub enum CSharpTypeEvidence {
    Exact(CSharpStringId),
    NullLiteral,
    Dynamic,
    #[default]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CSharpSymbolId(pub [u8; 32]);

impl CSharpSymbolId {
    pub fn parse(value: &str) -> Option<Self> {
        let hex = value.strip_prefix("cs:v1:")?;
        if hex.len() != 64 || hex.bytes().any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte)) {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (index, slot) in bytes.iter_mut().enumerate() {
            let offset = index * 2;
            *slot = u8::from_str_radix(&hex[offset..offset + 2], 16).ok()?;
        }
        Some(Self(bytes))
    }

    pub fn as_public_id(&self) -> String {
        let mut value = String::with_capacity(70);
        value.push_str("cs:v1:");
        for byte in self.0 {
            use std::fmt::Write;
            write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
        }
        value
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CSharpCallableKind {
    Method,
    Constructor,
}


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CSharpParameterShape {
    pub name: CSharpStringId,
    pub ty: CSharpStringId,
    pub ref_kind: CSharpRefKind,
    pub optional: bool,
    pub is_params: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CSharpCallableRecord {
    pub symbol_id: CSharpSymbolId,
    pub qualified_parent: CSharpStringId,
    pub name: CSharpStringId,
    pub kind: CSharpCallableKind,
    pub explicit_interface: Option<CSharpStringId>,
    pub has_body: bool,
    pub generic_arity: u16,
    pub parameters: Vec<CSharpParameterShape>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CSharpArgumentShape {
    pub name: Option<CSharpStringId>,
    pub ref_kind: CSharpRefKind,
    pub ty: CSharpTypeEvidence,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CSharpCallSiteShape {
    pub source_start: u32,
    pub source_end: u32,
    pub receiver: CSharpTypeEvidence,
    pub base_receiver: bool,
    pub method_generic_arity: Option<u16>,
    pub arguments: Vec<CSharpArgumentShape>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CSharpStringTable {
    values: Vec<String>,
    #[serde(skip)]
    lookup: HashMap<String, u32>,
}

impl CSharpStringTable {
    pub fn get(&self, id: CSharpStringId) -> Option<&str> {
        self.values.get(id.0 as usize).map(String::as_str)
    }

    fn intern(&mut self, value: String) -> CSharpStringId {
        self.ensure_lookup();
        if let Some(&id) = self.lookup.get(&value) {
            return CSharpStringId(id);
        }

        let id = self.values.len() as u32;
        self.values.push(value.clone());
        self.lookup.insert(value, id);
        CSharpStringId(id)
    }

    fn ensure_lookup(&mut self) {
        if self.lookup.len() == self.values.len() {
            return;
        }
        self.lookup = self.values.iter().enumerate()
            .map(|(index, value)| (value.clone(), index as u32))
            .collect();
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CSharpSemanticIndex {
    pub strings: CSharpStringTable,
    pub callables: Vec<CSharpCallableRecord>,
    pub def_to_callable: Vec<u32>,
    pub call_shapes_by_owner: HashMap<u32, Vec<CSharpCallSiteShape>>,
    pub symbol_to_definitions: HashMap<CSharpSymbolId, Vec<u32>>,
    pub extension_methods_by_file: HashMap<u32, HashMap<String, Vec<String>>>,
}

impl CSharpSemanticIndex {
    pub(crate) fn validate(
        &self,
        definition_count: usize,
        active_definitions: &HashSet<u32>,
        method_call_lengths: &HashMap<u32, usize>,
    ) -> Result<(), String> {
        if self.callables.is_empty()
            && self.def_to_callable.is_empty()
            && self.call_shapes_by_owner.is_empty()
            && self.symbol_to_definitions.is_empty()
        {
            return Ok(());
        }
        if self.def_to_callable.len() != definition_count {
            return Err(format!(
                "def_to_callable length {} does not match definitions {}",
                self.def_to_callable.len(),
                definition_count
            ));
        }

        for (definition_index, &encoded) in self.def_to_callable.iter().enumerate() {
            if encoded == 0 {
                continue;
            }
            let callable_index = encoded.checked_sub(1).unwrap() as usize;
            let Some(callable) = self.callables.get(callable_index) else {
                return Err(format!("callable id {encoded} is out of bounds"));
            };
            let definition_index = definition_index as u32;
            if !active_definitions.contains(&definition_index) {
                return Err(format!("inactive definition {definition_index} has a callable"));
            }
            if !self.symbol_to_definitions.get(&callable.symbol_id)
                .is_some_and(|occurrences| occurrences.contains(&definition_index))
            {
                return Err(format!("callable symbol is missing definition {definition_index}"));
            }
        }

        for (callable_index, callable) in self.callables.iter().enumerate() {
            let qualified_parent = self.string(callable.qualified_parent, "qualified parent")?;
            let name = self.string(callable.name, "callable name")?;
            let explicit_interface = callable.explicit_interface
                .map(|value| self.string(value, "explicit interface"))
                .transpose()?;
            let mut parameters = Vec::with_capacity(callable.parameters.len());
            for parameter in &callable.parameters {
                self.string(parameter.name, "parameter name")?;
                let ty = self.string(parameter.ty, "parameter type")?;
                parameters.push((ty, parameter.ref_kind));
            }
            let expected = compute_symbol_id(
                qualified_parent,
                name,
                explicit_interface,
                callable.kind,
                callable.generic_arity,
                parameters.iter().copied(),
                parameters.len(),
            );
            if callable.symbol_id != expected {
                return Err(format!("callable {callable_index} symbol hash mismatch"));
            }
        }

        for (&owner, shapes) in &self.call_shapes_by_owner {
            if !active_definitions.contains(&owner) {
                return Err(format!("call shapes reference inactive owner {owner}"));
            }
            let Some(&call_count) = method_call_lengths.get(&owner) else {
                return Err(format!("call shapes owner {owner} has no method calls"));
            };
            if shapes.len() != call_count {
                return Err(format!(
                    "call shapes owner {owner} has {} shapes for {call_count} calls",
                    shapes.len()
                ));
            }
            for shape in shapes {
                self.validate_evidence(&shape.receiver)?;
                for argument in &shape.arguments {
                    if let Some(name) = argument.name {
                        self.string(name, "argument name")?;
                    }
                    self.validate_evidence(&argument.ty)?;
                }
            }
        }

        for (symbol_id, occurrences) in &self.symbol_to_definitions {
            if occurrences.is_empty() {
                return Err("symbol has no definition occurrences".to_string());
            }
            for &definition_index in occurrences {
                if !active_definitions.contains(&definition_index) {
                    return Err(format!("symbol references inactive definition {definition_index}"));
                }
                let Some(callable) = self.callable_for_definition(definition_index) else {
                    return Err(format!("symbol definition {definition_index} has no callable"));
                };
                if callable.symbol_id != *symbol_id {
                    return Err(format!("symbol definition {definition_index} points to another symbol"));
                }
            }
        }
        Ok(())
    }

    fn string(&self, id: CSharpStringId, role: &str) -> Result<&str, String> {
        self.strings.get(id).ok_or_else(|| {
            format!("{role} string id {} is out of bounds", id.0)
        })
    }

    fn validate_evidence(&self, evidence: &CSharpTypeEvidence) -> Result<(), String> {
        if let CSharpTypeEvidence::Exact(id) = evidence {
            self.string(*id, "type evidence")?;
        }
        Ok(())
    }


    pub fn callable_for_definition(&self, def_idx: u32) -> Option<&CSharpCallableRecord> {
        let encoded = *self.def_to_callable.get(def_idx as usize)?;
        encoded.checked_sub(1)
            .and_then(|id| self.callables.get(id as usize))
    }

    pub fn call_shape(&self, owner_def_idx: u32, ordinal: usize) -> Option<&CSharpCallSiteShape> {
        self.call_shapes_by_owner.get(&owner_def_idx)?.get(ordinal)
    }

    pub fn symbol_id_for_definition(&self, def_idx: u32) -> Option<CSharpSymbolId> {
        self.callable_for_definition(def_idx).map(|callable| callable.symbol_id)
    }

    pub fn definitions_for_symbol(&self, symbol_id: CSharpSymbolId) -> &[u32] {
        self.symbol_to_definitions.get(&symbol_id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn apply_file_contribution(
        &mut self,
        base_def_idx: u32,
        file_id: u32,
        definition_count: usize,
        contribution: CSharpFileContribution,
    ) {
        let required = base_def_idx as usize + definition_count;
        self.def_to_callable.resize(required, 0);
        if contribution.extension_methods.is_empty() {
            self.extension_methods_by_file.remove(&file_id);
        } else {
            self.extension_methods_by_file.insert(file_id, contribution.extension_methods);
        }

        for callable in contribution.callables {
            if callable.local_def_idx >= definition_count {
                continue;
            }
            let symbol_id = callable.symbol_id();
            let global_def_idx = base_def_idx + callable.local_def_idx as u32;
            let record = CSharpCallableRecord {
                symbol_id,
                qualified_parent: self.strings.intern(callable.qualified_parent),
                name: self.strings.intern(callable.name),
                kind: callable.kind,
                explicit_interface: callable.explicit_interface.map(|qualifier| {
                    self.strings.intern(qualifier)
                }),
                has_body: callable.has_body,
                generic_arity: callable.generic_arity,
                parameters: callable.parameters.into_iter().map(|parameter| CSharpParameterShape {
                    name: self.strings.intern(parameter.name),
                    ty: self.strings.intern(parameter.ty),
                    ref_kind: parameter.ref_kind,
                    optional: parameter.optional,
                    is_params: parameter.is_params,
                }).collect(),
            };
            let callable_id = self.callables.len() as u32;
            self.callables.push(record);
            self.def_to_callable[global_def_idx as usize] = callable_id + 1;
            self.symbol_to_definitions.entry(symbol_id).or_default().push(global_def_idx);
        }

        for call_sites in contribution.call_sites {
            if call_sites.local_def_idx >= definition_count {
                continue;
            }
            let owner = base_def_idx + call_sites.local_def_idx as u32;
            let shapes = call_sites.shapes.into_iter()
                .map(|shape| self.intern_call_shape(shape))
                .collect();
            self.call_shapes_by_owner.insert(owner, shapes);
        }
    }

    pub(crate) fn remove_file_contribution(
        &mut self,
        file_id: u32,
        definitions: &HashSet<u32>,
    ) {
        self.extension_methods_by_file.remove(&file_id);
        for &def_idx in definitions {
            if let Some(symbol_id) = self.symbol_id_for_definition(def_idx) {
                let remove_symbol = self.symbol_to_definitions.get_mut(&symbol_id)
                    .is_some_and(|occurrences| {
                        occurrences.retain(|&occurrence| occurrence != def_idx);
                        occurrences.is_empty()
                    });
                if remove_symbol {
                    self.symbol_to_definitions.remove(&symbol_id);
                }
            }
            if let Some(slot) = self.def_to_callable.get_mut(def_idx as usize) {
                *slot = 0;
            }
            self.call_shapes_by_owner.remove(&def_idx);
        }
    }

    pub(crate) fn merged_extension_methods(&self) -> HashMap<String, Vec<String>> {
        let mut merged: HashMap<String, Vec<String>> = HashMap::new();
        for contribution in self.extension_methods_by_file.values() {
            for (method, classes) in contribution {
                merged.entry(method.clone()).or_default().extend(classes.iter().cloned());
            }
        }
        for classes in merged.values_mut() {
            classes.sort();
            classes.dedup();
        }
        merged
    }


    pub(crate) fn remap_definitions(
        &mut self,
        remap: &HashMap<u32, u32>,
        definition_count: usize,
    ) {
        let old_def_to_callable = std::mem::take(&mut self.def_to_callable);
        let old_callables = std::mem::take(&mut self.callables);
        let mut def_to_callable = vec![0; definition_count];
        let mut callables = Vec::new();
        let mut callable_remap: HashMap<u32, u32> = HashMap::new();
        let mut remap_entries: Vec<_> = remap.iter()
            .map(|(&old_idx, &new_idx)| (old_idx, new_idx))
            .collect();
        remap_entries.sort_by_key(|&(_, new_idx)| new_idx);
        for (old_idx, new_idx) in remap_entries {
            let Some(&encoded) = old_def_to_callable.get(old_idx as usize) else {
                continue;
            };
            let Some(old_callable_index) = encoded.checked_sub(1) else {
                continue;
            };
            let new_callable_index = if let Some(&existing) = callable_remap.get(&old_callable_index) {
                existing
            } else {
                let Some(callable) = old_callables.get(old_callable_index as usize) else {
                    continue;
                };
                let new_callable_index = callables.len() as u32;
                callables.push(callable.clone());
                callable_remap.insert(old_callable_index, new_callable_index);
                new_callable_index
            };
            def_to_callable[new_idx as usize] = new_callable_index + 1;
        }
        self.def_to_callable = def_to_callable;
        self.callables = callables;
        self.call_shapes_by_owner = self.call_shapes_by_owner.drain()
            .filter_map(|(old_idx, shapes)| remap.get(&old_idx).map(|&new_idx| (new_idx, shapes)))
            .collect();
        self.symbol_to_definitions = self.symbol_to_definitions.drain()
            .filter_map(|(symbol_id, occurrences)| {
                let remapped: Vec<_> = occurrences.into_iter()
                    .filter_map(|old_idx| remap.get(&old_idx).copied())
                    .collect();
                (!remapped.is_empty()).then_some((symbol_id, remapped))
            })
            .collect();
    }

    fn intern_call_shape(&mut self, shape: CSharpLocalCallSiteShape) -> CSharpCallSiteShape {
        CSharpCallSiteShape {
            source_start: shape.source_start,
            source_end: shape.source_end,
            receiver: self.intern_evidence(shape.receiver),
            base_receiver: shape.base_receiver,
            method_generic_arity: shape.method_generic_arity,
            arguments: shape.arguments.into_iter().map(|argument| CSharpArgumentShape {
                name: argument.name.map(|name| self.strings.intern(name)),
                ref_kind: argument.ref_kind,
                ty: self.intern_evidence(argument.ty),
            }).collect(),
        }
    }

    fn intern_evidence(&mut self, evidence: CSharpLocalTypeEvidence) -> CSharpTypeEvidence {
        match evidence {
            CSharpLocalTypeEvidence::Exact(ty) => CSharpTypeEvidence::Exact(self.strings.intern(ty)),
            CSharpLocalTypeEvidence::NullLiteral => CSharpTypeEvidence::NullLiteral,
            CSharpLocalTypeEvidence::Dynamic => CSharpTypeEvidence::Dynamic,
            CSharpLocalTypeEvidence::Unknown => CSharpTypeEvidence::Unknown,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CSharpFileContribution {
    pub extension_methods: HashMap<String, Vec<String>>,
    pub callables: Vec<CSharpLocalCallable>,
    pub call_sites: Vec<CSharpLocalCallSites>,
}

#[derive(Debug, Clone)]
pub struct CSharpLocalCallable {
    pub local_def_idx: usize,
    pub qualified_parent: String,
    pub name: String,
    pub kind: CSharpCallableKind,
    pub explicit_interface: Option<String>,
    pub has_body: bool,
    pub generic_arity: u16,
    pub parameters: Vec<CSharpLocalParameterShape>,
}

impl CSharpLocalCallable {
    fn symbol_id(&self) -> CSharpSymbolId {
        compute_symbol_id(
            &self.qualified_parent,
            &self.name,
            self.explicit_interface.as_deref(),
            self.kind,
            self.generic_arity,
            self.parameters.iter().map(|parameter| {
                (parameter.ty.as_str(), parameter.ref_kind)
            }),
            self.parameters.len(),
        )
    }
}

fn compute_symbol_id<'a>(
    qualified_parent: &str,
    name: &str,
    explicit_interface: Option<&str>,
    kind: CSharpCallableKind,
    generic_arity: u16,
    parameters: impl Iterator<Item = (&'a str, CSharpRefKind)>,
    parameter_count: usize,
) -> CSharpSymbolId {
    let mut hasher = Sha256::new();
    hasher.update(b"xray-csharp-callable-v1\0");
    hash_string(&mut hasher, qualified_parent);
    hash_string(&mut hasher, name);
    if let Some(qualifier) = explicit_interface {
        hasher.update([1]);
        hash_string(&mut hasher, qualifier);
    } else {
        hasher.update([0]);
    }
    hasher.update([match kind {
        CSharpCallableKind::Method => 1,
        CSharpCallableKind::Constructor => 2,
    }]);
    hasher.update(generic_arity.to_le_bytes());
    hasher.update((parameter_count as u32).to_le_bytes());
    for (parameter_type, ref_kind) in parameters {
        hash_string(&mut hasher, parameter_type);
        hasher.update([u8::from(ref_kind != CSharpRefKind::None)]);
    }
    CSharpSymbolId(hasher.finalize().into())
}

fn hash_string(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u32).to_le_bytes());
    hasher.update(value.as_bytes());
}

#[derive(Debug, Clone)]
pub struct CSharpLocalParameterShape {
    pub name: String,
    pub ty: String,
    pub ref_kind: CSharpRefKind,
    pub optional: bool,
    pub is_params: bool,
}

#[derive(Debug, Clone)]
pub struct CSharpLocalCallSites {
    pub local_def_idx: usize,
    pub shapes: Vec<CSharpLocalCallSiteShape>,
}

#[derive(Debug, Clone)]
pub struct CSharpLocalCallSiteShape {
    pub source_start: u32,
    pub source_end: u32,
    pub receiver: CSharpLocalTypeEvidence,
    pub base_receiver: bool,
    pub method_generic_arity: Option<u16>,
    pub arguments: Vec<CSharpLocalArgumentShape>,
}

#[derive(Debug, Clone)]
pub struct CSharpLocalArgumentShape {
    pub name: Option<String>,
    pub ref_kind: CSharpRefKind,
    pub ty: CSharpLocalTypeEvidence,
}

#[cfg_attr(not(feature = "lang-csharp"), allow(dead_code))]
#[derive(Debug, Clone, Default)]
pub enum CSharpLocalTypeEvidence {
    Exact(String),
    NullLiteral,
    Dynamic,
    #[default]
    Unknown,
}
