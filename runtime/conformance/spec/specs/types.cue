package conformance

// Labels are quoted where they'd shadow a predeclared type (`bool`, `string`,
// `uint8`...), which a field's value would then refer to instead.

// A type a `decode` case reads into. It covers every shape a generated Rust
// type can have. `self` means something only inside a `recursive` type.
#Type:
	"bool" | "u8" | "u16" | "u32" | "u64" | "u128" |
	"i8" | "i16" | "i32" | "i64" | "i128" |
	"f32" | "f64" | "string" | "char" | "unit" | "self" |
	close({bounded_string!: close({max!: uint32})}) |
	close({option!: #Type}) |
	close({array!: #Type}) |
	close({fixed!: close({of!: #Type, len!: uint})}) |
	close({bounded_array!: close({of!: #Type, max!: uint32})}) |
	close({recursive!: close({max_depth!: uint32, of!: #Type})}) |
	close({"struct"!: close({name!: string, fields!: [...[string, #Type]]})}) |
	close({"enum"!: close({name!: string, variants!: [...string]})})

// A value as a vector states it: typed, so a runtime knows which width or
// shape to encode.
#Value:
	close({"bool"!: bool}) |
	close({u8!: uint8}) |
	close({u16!: uint16}) |
	close({u32!: uint32}) |
	close({u64!: #Unsigned}) |
	close({u128!: #Unsigned}) |
	close({i8!: int8}) |
	close({i16!: int16}) |
	close({i32!: int32}) |
	close({i64!: #Signed}) |
	close({i128!: #Signed}) |
	close({f32!: number}) |
	close({f64!: number}) |
	close({"string"!: string}) |
	close({char!: =~"^[\\s\\S]$"}) |
	close({array!: [...#Value]}) |
	close({tuple!: [...#Value]}) |
	close({option!: null | #Value}) |
	close({"struct"!: close({name!: string, fields!: [...[string, #Value]]})}) |
	close({unit_variant!: close({"enum"!: string, index!: uint32, variant!: string})}) |
	close({unit!: null})

// A type a `size` case gives the range of: `#Type`, and `fixed_string` for
// `char[N]`, a string of exactly N characters. The sizes of `string`,
// `bounded_string` and `fixed_string` count characters (Unicode scalar
// values), as `char[max N]` does.
#SizeType:
	"bool" | "u8" | "u16" | "u32" | "u64" | "u128" |
	"i8" | "i16" | "i32" | "i64" | "i128" |
	"f32" | "f64" | "string" | "char" | "unit" | "self" |
	close({bounded_string!: close({max!: uint32})}) |
	close({fixed_string!: close({len!: uint32})}) |
	close({option!: #SizeType}) |
	close({array!: #SizeType}) |
	close({fixed!: close({of!: #SizeType, len!: uint})}) |
	close({bounded_array!: close({of!: #SizeType, max!: uint32})}) |
	close({recursive!: close({max_depth!: uint32, of!: #SizeType})}) |
	close({"struct"!: close({name!: string, fields!: [...[string, #SizeType]]})}) |
	close({"enum"!: close({name!: string, variants!: [...string]})})
