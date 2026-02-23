if (args.length === 0) {
    print("Usage: rm <file/directory>");
} else {
    try {
        sys.fs.delete(args[0]);
    } catch (e) {
        print("Error: " + e);
    }
}
