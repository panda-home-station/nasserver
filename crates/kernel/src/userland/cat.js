if (args.length === 0) {
    print("Usage: cat <file>");
} else {
    try {
        const content = sys.fs.readFile(args[0]);
        print(content);
    } catch (e) {
        print("Error: " + e);
    }
}
