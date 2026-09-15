use prost::Message;

/// Compact descriptor baseline: pin names, tags, wire types, cardinality and oneofs.
pub fn schema_manifest() -> String {
    let descriptor =
        prost_types::FileDescriptorSet::decode(util_lib::protocol::DESCRIPTOR).unwrap();
    let mut lines = Vec::new();
    for file in descriptor.file {
        lines.push(format!(
            "package {} syntax {}",
            file.package(),
            file.syntax()
        ));
        for message in file.message_type {
            for field in &message.field {
                let oneof = field
                    .oneof_index
                    .map(|i| message.oneof_decl[i as usize].name())
                    .unwrap_or("-");
                lines.push(format!(
                    "{}.{} tag={} type={:?}:{} label={:?} oneof={} optional={}",
                    message.name(),
                    field.name(),
                    field.number(),
                    field.r#type(),
                    field.type_name(),
                    field.label(),
                    oneof,
                    field.proto3_optional()
                ));
            }
            if message.field.is_empty() {
                lines.push(format!("{} empty", message.name()));
            }
        }
        for enumeration in file.enum_type {
            for value in &enumeration.value {
                lines.push(format!(
                    "{}.{}={}",
                    enumeration.name(),
                    value.name(),
                    value.number()
                ));
            }
        }
    }
    lines.sort();
    lines.join("\n") + "\n"
}
