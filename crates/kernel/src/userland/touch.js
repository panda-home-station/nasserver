if (args.length === 0) {
    print("Usage: touch <file>");
} else {
    try {
        sys.fs.writeFile(args[0], "", true);
    } catch (e) {
        print("Error: " + e);
    }
}
