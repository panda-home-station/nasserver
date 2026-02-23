if (args.length < 2) {
    print("Usage: cp <source> <dest>");
} else {
    try {
        const content = sys.fs.readFile(args[0]);
        sys.fs.writeFile(args[1], content);
    } catch (e) {
        print("Error: " + e);
    }
}
