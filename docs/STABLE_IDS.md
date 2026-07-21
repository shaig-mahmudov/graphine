# Java stable symbol IDs

Public IDs are semantic strings, never Eclipse JDT binding keys:

```text
module:<maven-artifact>
package:com.example
type:com.example.Outer.Nested
method:com.example.Service#send(java.lang.String,int[])
constructor:com.example.Service#<init>(java.lang.String)
field:com.example.Service#counter
```

Method and constructor parameters are ordered, comma-separated fully qualified types. Varargs normalize to arrays. Array dimensions are preserved. Nested types use dots. Parameterized types use erasure in IDs (`java.util.List`, not `java.util.List<String>`), while declaration generic parameters remain metadata. Primitive types keep Java spelling. Constructors always use `<init>`.

This erasure policy makes IDs stable across type-use substitutions and matches JVM overload identity except where Java source rules already prohibit an erasure collision. Overloads remain distinct by normalized parameter list. Return type is not part of a method ID because Java cannot overload by return type alone.

Source declarations and lightweight external nodes use the same grammar. IDs are validated again by Rust before ingestion.
