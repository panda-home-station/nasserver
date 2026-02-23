const path = args[0] || ".";
try {
    const files = sys.fs.readDir(path);
    files.forEach(f => {
        print(f.name + (f.type === "dir" ? "/" : ""));
    });
} catch (e) {
    print("Error: " + e);
}
