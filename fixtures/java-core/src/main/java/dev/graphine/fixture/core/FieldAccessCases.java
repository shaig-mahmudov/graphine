package dev.graphine.fixture.core;

public final class FieldAccessCases {
    private int counter;

    public int read() { return this.counter; }
    public void write(int value) { this.counter = value; }
    public void compound(int value) { this.counter += value; }
    public void increment() { this.counter++; }
}
