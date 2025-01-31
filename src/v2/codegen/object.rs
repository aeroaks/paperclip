//! Simplified objects for codegen.
//!
//! This contains the necessary objects for generating actual
//! API objects, their builders, impls, etc.

pub use super::impls::{ApiObjectBuilderImpl, ApiObjectImpl};

use super::emitter::{ANY_GENERIC_PARAMETER, FILE_MARKER};
use super::RUST_KEYWORDS;
use crate::v2::models::{Coder, CollectionFormat, HttpMethod, ParameterIn};
use heck::{CamelCase, SnekCase};
use lazy_static::lazy_static;
use regex::{Captures, Regex};

use std::collections::{BTreeMap, HashSet};
use std::fmt::{self, Display, Write};
use std::iter;
use std::sync::Arc;

lazy_static! {
    /// Regex for appropriate escaping in docs.
    static ref DOC_REGEX: Regex = Regex::new(r"\[|\]").expect("invalid doc regex?");
}

/// Represents a (simplified) Rust struct or enum.
#[derive(Default, Debug, Clone)]
pub struct ApiObject {
    /// Name of the struct (camel-cased).
    pub name: String,
    /// Description for this object (if any), to be used for docs.
    pub description: Option<String>,
    /// Path to this object from (generated) root module.
    pub path: String,
    /// List of fields.
    pub fields: Vec<ObjectField>,
    /// Paths with operations which address this object.
    pub paths: BTreeMap<String, PathOps>,
}

/// Operations in a path.
#[derive(Default, Debug, Clone)]
pub struct PathOps {
    /// Operations for this object and their associated requirements.
    pub req: BTreeMap<HttpMethod, OpRequirement>,
    /// Parameters required for all operations in this path.
    pub params: Vec<Parameter>,
}

/// Requirement for an object corresponding to some operation.
#[derive(Debug, Clone)]
pub struct OpRequirement {
    /// Operation ID (if it's provided in the schema).
    ///
    /// If there are multiple operations for the same path, then we
    /// attempt to use this.
    pub id: Option<String>,
    /// Description of this operation (if any), to be used for docs.
    pub description: Option<String>,
    /// Whether the operation is deprecated or not.
    pub deprecated: bool,
    /// Parameters required for this operation.
    pub params: Vec<Parameter>,
    /// Whether the object itself is required (in body) for this operation.
    pub body_required: bool,
    /// Whether this operation returns a list of the associated `ApiObject`.
    pub listable: bool,
    /// Response information for this operation.
    pub response: Response<String>,
    /// Preferred media range and encoder for the client. This is ignored for
    /// methods that don't accept a body. If there's no coder, then JSON
    /// encoding is assumed.
    pub encoding: Option<(String, Arc<Coder>)>,
    /// Preferred media range and decoder for the client. This is used only
    /// when objects make use of `Any` type. If there's no coder, then JSON
    /// encoding is assumed.
    pub decoding: Option<(String, Arc<Coder>)>,
}

#[derive(Default, Debug, Clone)]
pub struct Response<S> {
    /// Type path for this operation's response (if any). If this is empty,
    /// then we go for `Any`.
    pub ty_path: Option<S>,
    /// Whether the response contains an `Any`. This is useful when operations
    /// get bound to some other object.
    pub contains_any: bool,
}

impl<S> Response<S>
where
    S: AsRef<str>,
{
    /// Returns whether this response is a file.
    pub fn is_file(&self) -> bool {
        self.ty_path
            .as_ref()
            .map(|s| s.as_ref() == FILE_MARKER)
            .unwrap_or_default()
    }
}

/// Represents some parameter somewhere (header, path, query, etc.).
#[derive(Debug, Clone)]
pub struct Parameter {
    /// Name of the parameter.
    pub name: String,
    /// Description of this operation (if any), to be used for docs.
    pub description: Option<String>,
    /// Type of the parameter as a path.
    pub ty_path: String,
    /// Whether this parameter is required.
    pub required: bool,
    /// Where the parameter lives.
    pub presence: ParameterIn,
    /// If the parameter is an array of values, then the format for collecting them.
    pub delimiting: Vec<CollectionFormat>,
}

/// Represents a struct field.
#[derive(Debug, Clone)]
pub struct ObjectField {
    /// Name of the field.
    pub name: String,
    /// Type of the field as a path.
    pub ty_path: String,
    /// Description of this operation (if any), to be used for docs.
    pub description: Option<String>,
    /// Whether this field is required (i.e., not optional).
    pub is_required: bool,
    /// Whether this field's type "is" or "has" an `Any` type.
    pub needs_any: bool,
    /// Whether this field should be boxed.
    pub boxed: bool,
    /// Required fields of the "deepest" child type in the given definition.
    ///
    /// Now, what do I mean by "deepest"? For example, if we had `Vec<Vec<Vec<T>>>`
    /// or `Vec<BTreeMap<String, Vec<BTreeMap<String, T>>>>`, then "deepest" child
    /// type is T (as long as it's not a `Vec` or `BTreeMap`).
    ///
    /// To understand why we're doing this, see `ApiObjectBuilderImpl::write_builder_ty`
    /// and `ApiObjectBuilderImpl::write_value_map` functions.
    ///
    /// Yours sincerely.
    pub child_req_fields: Vec<String>,
}

impl ApiObject {
    /// Create an object with the given name.
    pub fn with_name<S>(name: S) -> Self
    where
        S: Into<String>,
    {
        ApiObject {
            name: name.into(),
            // NOTE: Even though `path` is empty, it'll be replaced by the emitter.
            ..Default::default()
        }
    }

    /// Writes `Any` as a generic parameter (including `<>`).
    pub(super) fn write_any_generic<F>(f: &mut F) -> fmt::Result
    where
        F: Write,
    {
        f.write_str("<")?;
        f.write_str(ANY_GENERIC_PARAMETER)?;
        f.write_str(">")
    }

    /// Writes the given string (if any) as Rust documentation into
    /// the given formatter.
    pub(super) fn write_docs<F, S>(stuff: Option<S>, f: &mut F, levels: usize) -> fmt::Result
    where
        F: Write,
        S: AsRef<str>,
    {
        let indent = iter::repeat(' ').take(levels * 4).collect::<String>();
        if let Some(desc) = stuff.as_ref() {
            desc.as_ref().split('\n').try_for_each(|line| {
                f.write_str("\n")?;
                f.write_str(&indent)?;
                f.write_str("///")?;
                if line.is_empty() {
                    return Ok(());
                }

                f.write_str(" ")?;
                f.write_str(
                    &DOC_REGEX
                        .replace_all(line, |c: &Captures| match &c[0] {
                            "[" => "\\[",
                            "]" => "\\]",
                            _ => unreachable!(),
                        })
                        .trim_end(),
                )
            })?;
            f.write_str("\n")?;
        }

        Ok(())
    }

    /// Returns whether this type is simple (i.e., not an object defined by us).
    #[inline]
    pub(super) fn is_simple_type(ty: &str) -> bool {
        !ty.contains("::") || ty.ends_with("Delimited")
    }

    /// Assuming that the given type "is" or "has" `Any`, this adds
    /// the appropriate generic parameter.
    fn write_field_with_any<F>(ty: &str, f: &mut F) -> fmt::Result
    where
        F: Write,
    {
        if let Some(i) = ty.find('<') {
            if ty[..i].ends_with("Vec") {
                f.write_str(&ty[..=i])?;
                Self::write_field_with_any(&ty[i + 1..ty.len() - 1], f)?;
            } else if ty[..i].ends_with("std::collections::BTreeMap") {
                f.write_str(&ty[..i + 9])?;
                Self::write_field_with_any(&ty[i + 9..ty.len() - 1], f)?;
            } else {
                unreachable!("no other generics expected.");
            }

            f.write_str(">")?;
            return Ok(());
        }

        f.write_str(ty)?;
        if !Self::is_simple_type(ty) {
            Self::write_any_generic(f)?;
        }

        Ok(())
    }
}

/// Represents a builder struct for some API object.
#[derive(Default, Debug, Clone)]
pub(super) struct ApiObjectBuilder<'a> {
    /// Index of this builder.
    pub idx: usize,
    /// Description if any, for docs.
    pub description: Option<&'a str>,
    /// Whether body is required for this builder.
    pub body_required: bool,
    /// Prefix for addressing stuff from crate root.
    pub helper_module_prefix: &'a str,
    /// Operation ID, if any.
    pub op_id: Option<&'a str>,
    /// Whether the operation is deprecated or not.
    pub deprecated: bool,
    /// HTTP method for the operation - all builders (other than object builders)
    /// have this.
    pub method: Option<HttpMethod>,
    /// Relative URL path - presence is same as HTTP method.
    pub rel_path: Option<&'a str>,
    /// Whether this operation returns a list object.
    pub is_list_op: bool,
    /// Response for this operation, if any.
    pub response: Response<&'a str>,
    /// Object to which this builder belongs to.
    pub object: &'a str,
    /// Encoding for the operation, if it's not JSON.
    pub encoding: Option<&'a (String, Arc<Coder>)>,
    /// Decoding for the operation, if it's not JSON.
    ///
    /// **NOTE:** We use this to set the `Accept` header for operations
    /// which return objects that are (or have) `Any` type.
    pub decoding: Option<&'a (String, Arc<Coder>)>,
    /// Whether there are multiple builders for this object.
    pub multiple_builders_exist: bool,
    /// Fields in this builder.
    pub fields: &'a [ObjectField],
    /// Parameters global to this URL path.
    pub global_params: &'a [Parameter],
    /// Parameters local to this operation.
    pub local_params: &'a [Parameter],
    /// Whether this builder is generic over `Any` type.
    pub needs_any: bool,
}

/// The property we're dealing with.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum Property {
    RequiredField,
    OptionalField,
    RequiredParam,
    OptionalParam,
}

impl Property {
    /// Whether this property is required.
    pub(super) fn is_required(self) -> bool {
        match self {
            Property::RequiredField | Property::RequiredParam => true,
            _ => false,
        }
    }

    /// Checks whether this property is a parameter.
    pub(super) fn is_parameter(self) -> bool {
        match self {
            Property::RequiredParam | Property::OptionalParam => true,
            _ => false,
        }
    }

    /// Checks whether this property is a field.
    pub(super) fn is_field(self) -> bool {
        match self {
            Property::RequiredField | Property::OptionalField => true,
            _ => false,
        }
    }
}

/// See `ApiObjectBuilder::write_generics_if_necessary`
pub(super) enum TypeParameters<'a> {
    Generic,
    ChangeOne(&'a str),
    ReplaceAll,
    ChangeAll,
}

/// Represents a Rust struct field (could be actual object field or a parameter).
#[derive(Debug, Clone)]
pub(super) struct StructField<'a> {
    /// Name of this field (case unspecified).
    pub name: &'a str,
    /// Type of this field.
    pub ty: &'a str,
    /// What this field represents.
    pub prop: Property,
    /// Description for this field (if any), for docs.
    pub desc: Option<&'a str>,
    /// Whether this field had a collision (i.e., between parameter and object field)
    pub overridden: bool,
    /// Required fields of child needed for this field. If they exist, then we
    /// switch to requiring a builder.
    pub strict_child_fields: &'a [String],
    /// Delimiting for array values (if it is a parameter).
    pub delimiting: &'a [CollectionFormat],
    /// Location of the parameter (if it is a parameter).
    pub param_loc: Option<ParameterIn>,
    /// Whether this field "is" or "has" `Any` type. This is only
    /// applicable for object fields.
    pub needs_any: bool,
    /// Whether this field indicates a file upload.
    pub needs_file: bool,
}

impl<'a> ApiObjectBuilder<'a> {
    /// Name of the constructor function which creates this builder.
    pub fn constructor_fn_name(&self) -> Option<String> {
        match (self.op_id, self.method) {
            // If there's an operation ID, then we go for that ...
            (Some(id), _) => Some(id.to_snek_case()),
            // If there's a method and we *don't* have any collisions
            // (i.e., two or more paths for same object), then we default
            // to using the method ...
            (_, Some(meth)) if !self.multiple_builders_exist => {
                Some(meth.to_string().to_snek_case())
            }
            // If there's a method, then we go for numbered functions ...
            (_, Some(meth)) => {
                let mut name = meth.to_string().to_snek_case();
                if self.idx > 0 {
                    name.push('_');
                    name.push_str(&self.idx.to_string());
                }

                Some(name)
            }
            // We don't know what to do ...
            // FIXME: Use route and method to generate a name.
            _ => None,
        }
    }

    /// Returns an iterator of all fields and parameters required for the Rust builder struct.
    ///
    /// **NOTE:** The names yielded by this iterator are unique for a builder.
    /// If there's a collision between a path-specific parameter and an operation-specific
    /// parameter, then the latter overrides the former. If there's a collision between a field
    /// and a parameter, then the latter overrides the former.
    pub(super) fn struct_fields_iter(&self) -> impl Iterator<Item = StructField<'a>> + 'a {
        let body_required = self.body_required;
        let field_iter = self.fields.iter().map(move |field| StructField {
            name: field.name.as_str(),
            ty: field.ty_path.as_str(),
            // We "require" the object fields only if the object itself is required.
            prop: if body_required && field.is_required {
                Property::RequiredField
            } else {
                Property::OptionalField
            },
            desc: field.description.as_ref().map(String::as_str),
            strict_child_fields: &*field.child_req_fields,
            param_loc: None,
            overridden: false,
            needs_any: field.needs_any,
            needs_file: field.ty_path == FILE_MARKER,
            delimiting: &[],
        });

        let param_iter = self
            .global_params
            .iter()
            .chain(self.local_params.iter())
            .scan(HashSet::new(), |set, param| {
                // Local parameters override global parameters.
                if set.contains(&param.name) {
                    // Workaround because `scan` stops when it encounters
                    // `None`, but we want filtering.
                    Some(None)
                } else {
                    set.insert(&param.name);
                    Some(Some(StructField {
                        name: param.name.as_str(),
                        ty: param.ty_path.as_str(),
                        prop: if param.required {
                            Property::RequiredParam
                        } else {
                            Property::OptionalParam
                        },
                        desc: param.description.as_ref().map(String::as_str),
                        strict_child_fields: &[] as &[_],
                        param_loc: Some(param.presence),
                        overridden: false,
                        needs_any: false,
                        needs_file: param.ty_path == FILE_MARKER,
                        delimiting: &param.delimiting,
                    }))
                }
            })
            .filter_map(|p| p);

        let mut fields = vec![];
        // Check parameter-field collisions.
        for field in param_iter.chain(field_iter) {
            if let Some(v) = fields
                .iter_mut()
                .find(|f: &&mut StructField<'_>| f.name == field.name)
            {
                if v.ty == field.ty {
                    v.overridden = true;
                }

                // We don't know what we should do when we encounter
                // parameter-field collision and they have different types.
                continue;
            }

            fields.push(field);
        }

        fields.into_iter()
    }

    /// Write this builder's name into the given formatter.
    pub(super) fn write_name<F>(&self, f: &mut F) -> fmt::Result
    where
        F: Write,
    {
        f.write_str(&self.object)?;
        if let Some(method) = self.method {
            write!(f, "{}", method)?;
        }

        f.write_str("Builder")?;
        if self.idx > 0 {
            f.write_str(&self.idx.to_string())?;
        }

        Ok(())
    }

    /// Writes generic parameters, if needed.
    ///
    /// Also takes an enum to specify whether the one/all/none of the parameters
    /// should make use of actual types.
    pub(super) fn write_generics_if_necessary<F>(
        &self,
        f: &mut F,
        any_value: Option<&str>,
        params: TypeParameters<'_>,
    ) -> Result<usize, fmt::Error>
    where
        F: Write,
    {
        let mut num_generics = 0;
        // Inspect fields and parameters and write generics.
        self.struct_fields_iter()
            .filter(|f| f.prop.is_required())
            .enumerate()
            .try_for_each(|(i, field)| {
                num_generics += 1;
                if i == 0 {
                    f.write_str("<")?;
                } else {
                    f.write_str(", ")?;
                }

                match params {
                    // If the name matches, then change that unit type to `{Name}Exists`
                    TypeParameters::ChangeOne(n) if field.name == n => {
                        f.write_str(self.helper_module_prefix)?;
                        f.write_str("generics::")?;
                        f.write_str(&field.name.to_camel_case())?;
                        return f.write_str("Exists");
                    }
                    // All names should be changed to `{Name}Exists`
                    TypeParameters::ChangeAll => {
                        f.write_str(self.helper_module_prefix)?;
                        f.write_str("generics::")?;
                        f.write_str(&field.name.to_camel_case())?;
                        return f.write_str("Exists");
                    }
                    // All names should be reset to `Missing{Name}`
                    TypeParameters::ReplaceAll => {
                        f.write_str(self.helper_module_prefix)?;
                        f.write_str("generics::")?;
                        f.write_str("Missing")?;
                    }
                    _ => (),
                }

                f.write_str(&field.name.to_camel_case())
            })?;

        if self.needs_any {
            if num_generics > 0 {
                f.write_str(", ")?;
            } else {
                f.write_str("<")?;
            }

            f.write_str(any_value.unwrap_or(ANY_GENERIC_PARAMETER))?;
            num_generics += 1;
        }

        if num_generics > 0 {
            f.write_str(">")?;
        }

        Ok(num_generics)
    }

    /// Returns whether this builder will have at least one field.
    pub(super) fn has_atleast_one_field(&self) -> bool {
        self.struct_fields_iter()
            .any(|f| f.prop.is_parameter() || f.prop.is_required())
    }

    /// Returns whether a separate container is needed for the builder struct.
    pub(super) fn needs_container(&self) -> bool {
        // This is perhaps one of those important blocks, because this
        // decides whether to mark builder structs as `repr(transparent)`
        // (for unsafely transmuting). It's UB to transmute `repr(Rust)`
        // structs, so we put stuff into a container and transmute
        // whenever a builder:
        //
        // - Has at least one operation parameter that's required (or)
        // - Has a body with at least one field that's required and the
        // operation has at least one parameter.
        //
        // Because, we need `mem::transmute` only when we use phantom fields
        // and we use phantom fields only when there's a "required" constraint.
        // And, we don't need a container if there's just a body (i.e., no params),
        // because we can transmute the builder directly.

        self.local_params
            .iter()
            .chain(self.global_params.iter())
            .any(|p| p.required)
            || (self.body_required
                && self.fields.iter().any(|f| f.is_required)
                && self.local_params.len() + self.global_params.len() > 0)
    }

    /// Write this builder's container name into the given formatter.
    pub(super) fn write_container_name<F>(&self, f: &mut F) -> fmt::Result
    where
        F: Write,
    {
        self.write_name(f)?;
        f.write_str("Container")
    }

    /// Given the helper module prefix, type and delimiters for that type,
    /// wraps the type (if needed) and writes the old or new type to the given formatter.
    pub(super) fn write_wrapped_ty<F>(
        module_prefix: &str,
        ty: &str,
        delims: &[CollectionFormat],
        f: &mut F,
    ) -> fmt::Result
    where
        F: fmt::Write,
    {
        if !ty.contains("Vec") {
            return f.write_str(ty);
        }

        // In parameters, we're limited to basic types and arrays,
        // so we can assume that whatever `<>` we encounter, they're
        // all for `Vec`.
        let delim_ty = String::from(module_prefix) + "util::Delimited";
        let mut ty = ty.replace("Vec", &delim_ty);
        let mut new_ty = String::new();
        // From the reverse, because we replace from inside out.
        let mut delim_idx = delims.len();
        while let Some(idx) = ty.find('>') {
            delim_idx -= 1;
            new_ty.push_str(&ty[..idx]);
            new_ty.push_str(", ");
            write!(
                &mut new_ty,
                "{}util::{:?}",
                module_prefix, delims[delim_idx]
            )?;
            new_ty.push('>');
            if idx == ty.len() - 1 {
                break;
            }

            ty = ty[idx + 1..].into();
        }

        f.write_str(&new_ty)
    }

    /// Writes the body field into the formatter if required.
    fn write_body_field_if_required<F>(&self, f: &mut F) -> fmt::Result
    where
        F: Write,
    {
        if self.body_required {
            // We address with 'self::' because it's possible for body type
            // to collide with type parameters (if any).
            f.write_str("\n    body: self::")?;
            f.write_str(&self.object)?;
            if self.needs_any {
                ApiObject::write_any_generic(f)?;
            }

            f.write_str(",")?;
        }

        Ok(())
    }

    /// Writes the parameter into the formatter if required.
    fn write_parameter_if_required<F>(
        &self,
        prop: Property,
        name: &str,
        ty: &str,
        delims: &[CollectionFormat],
        f: &mut F,
    ) -> fmt::Result
    where
        F: Write,
    {
        if !prop.is_parameter() {
            return Ok(());
        }

        f.write_str("\n    param_")?;
        f.write_str(&name)?;
        f.write_str(": Option<")?;
        if ty == FILE_MARKER {
            f.write_str("std::path::PathBuf")?;
        } else {
            Self::write_wrapped_ty(self.helper_module_prefix, ty, delims, f)?;
        }

        f.write_str(">,")
    }
}

impl<'a> Display for ApiObjectBuilder<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("/// Builder ")?;
        if let (Some(name), Some(m)) = (self.constructor_fn_name(), self.method) {
            f.write_str("created by [`")?;
            f.write_str(&self.object)?;
            f.write_str("::")?;
            f.write_str(&name)?;
            f.write_str("`](./struct.")?;
            f.write_str(&self.object)?;
            f.write_str(".html#method.")?;
            f.write_str(&name)?;
            f.write_str(") method for a `")?;
            f.write_str(&m.to_string().to_uppercase())?;
            f.write_str("` operation associated with `")?;
            f.write_str(&self.object)?;
            f.write_str("`.\n")?;
        } else {
            f.write_str("for [`")?;
            f.write_str(&self.object)?;
            f.write_str("`](./struct.")?;
            f.write_str(&self.object)?;
            f.write_str(".html) object.\n")?;
        }

        // If the builder "needs" parameters/fields, then we go for a separate
        // container which holds both the body (if any) and the parameters,
        // so that we can make the actual builder `#[repr(transparent)]`
        // for safe transmuting.
        let needs_container = self.needs_container();
        if needs_container {
            f.write_str("#[repr(transparent)]\n")?;
        }

        f.write_str("#[derive(Debug, Clone)]\npub struct ")?;
        self.write_name(f)?;
        self.write_generics_if_necessary(f, None, TypeParameters::Generic)?;

        // If structs don't have any fields, then we go for unit structs.
        let has_fields = self.has_atleast_one_field();

        if has_fields || self.body_required || needs_container {
            f.write_str(" {")?;
        }

        let mut container = String::new();
        if needs_container {
            container.push_str("#[derive(Debug, Default, Clone)]\nstruct ");
            self.write_container_name(&mut container)?;
            if self.needs_any {
                ApiObject::write_any_generic(&mut container)?;
            }

            container.push_str(" {");
            self.write_body_field_if_required(&mut container)?;

            f.write_str("\n    inner: ")?;
            self.write_container_name(f)?;
            if self.needs_any {
                ApiObject::write_any_generic(f)?;
            }

            f.write_str(",")?;
        } else {
            self.write_body_field_if_required(f)?;
        }

        // Write struct fields and the associated markers if needed.
        self.struct_fields_iter().try_for_each(|field| {
            let (cc, sk) = (field.name.to_camel_case(), field.name.to_snek_case());
            if needs_container {
                self.write_parameter_if_required(
                    field.prop,
                    &sk,
                    field.ty,
                    &field.delimiting,
                    &mut container,
                )?;
            } else {
                self.write_parameter_if_required(field.prop, &sk, field.ty, &field.delimiting, f)?;
            }

            if field.prop.is_required() {
                f.write_str("\n    ")?;
                if field.prop.is_parameter() {
                    f.write_str("_param")?;
                }

                f.write_str("_")?;
                f.write_str(&sk)?;
                f.write_str(": ")?;
                f.write_str("core::marker::PhantomData<")?;
                f.write_str(&cc)?;
                f.write_str(">,")?;
            }

            Ok(())
        })?;

        if has_fields || self.body_required {
            f.write_str("\n}\n")?;
        } else {
            f.write_str(";\n")?;
        }

        if needs_container {
            f.write_str("\n")?;
            f.write_str(&container)?;
            f.write_str("\n}\n")?;
        }

        Ok(())
    }
}

impl Display for ApiObject {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        ApiObject::write_docs(self.description.as_ref(), f, 0)?;

        f.write_str("#[derive(Debug, Default, Clone, Deserialize, Serialize)]")?;
        f.write_str("\npub struct ")?;
        f.write_str(&self.name)?;
        if self.fields.iter().any(|f| f.needs_any) {
            ApiObject::write_any_generic(f)?;
        }

        f.write_str(" {")?;

        self.fields.iter().try_for_each(|field| {
            let mut new_name = field.name.to_snek_case();
            // Check if the field matches a Rust keyword and add '_' suffix.
            if RUST_KEYWORDS.iter().any(|&k| k == new_name) {
                new_name.push('_');
            }

            ApiObject::write_docs(field.description.as_ref(), f, 1)?;
            if field.description.is_none() {
                f.write_str("\n")?;
            }

            f.write_str("    ")?;
            if new_name != field.name.as_str() {
                f.write_str("#[serde(rename = \"")?;
                f.write_str(&field.name)?;
                f.write_str("\")]\n    ")?;
            }

            f.write_str("pub ")?;
            f.write_str(&new_name)?;
            f.write_str(": ")?;
            if !field.is_required {
                f.write_str("Option<")?;
            }

            if field.boxed {
                f.write_str("Box<")?;
            }

            if field.needs_any {
                Self::write_field_with_any(&field.ty_path, f)?;
            } else {
                f.write_str(&field.ty_path)?;
            }

            if field.boxed {
                f.write_str(">")?;
            }

            if !field.is_required {
                f.write_str(">")?;
            }

            f.write_str(",")?;
            Ok(())
        })?;

        if !self.fields.is_empty() {
            f.write_str("\n")?;
        }

        f.write_str("}\n")
    }
}
