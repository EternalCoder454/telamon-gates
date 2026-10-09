use cxx_qt_build::CxxQtBuilder;

fn main() {
    // Generates the C++ for the QObject bridges and compiles it into the Rust
    // static library. Qt is found through $QMAKE (CMake sets it).
    CxxQtBuilder::new()
        .file("src/chat.rs")
        .file("src/library.rs")
        .file("src/vram.rs")
        .file("src/models.rs")
        .build();
}
