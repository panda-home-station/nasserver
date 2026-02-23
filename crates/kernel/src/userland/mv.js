if (args.length < 2) {
    print("Usage: mv <source> <dest>");
} else {
    try {
        sys.fs.rename(args[0], args[1]);
    } catch (e) {
        print("Error: " + e);
    }
}
