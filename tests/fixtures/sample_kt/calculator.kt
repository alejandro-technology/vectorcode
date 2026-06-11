package com.example.calculator

interface Calculatable {
    fun add(n: Int): Int
    fun subtract(n: Int): Int
    fun result(): Int
}

class Calculator(initial: Int = 0, private val name: String = "default") : Calculatable {
    private var value: Int = initial
    private val history: MutableList<String> = mutableListOf()

    override fun add(n: Int): Int {
        value += n
        history.add("add($n) = $value")
        println("Adding $n, result: $value")
        return value
    }

    override fun subtract(n: Int): Int {
        value -= n
        history.add("subtract($n) = $value")
        return value
    }

    override fun result(): Int {
        return value
    }

    fun reset() {
        value = 0
        history.clear()
    }

    fun getHistory(): List<String> {
        return history.toList()
    }
}

object CalculatorFactory {
    fun create(): Calculator = Calculator(0)

    fun createWithInitial(value: Int): Calculator = Calculator(value)

    fun createNamed(name: String, initial: Int): Calculator {
        return Calculator(initial, name)
    }
}

enum class Operation(val symbol: String) {
    ADDITION("+"),
    SUBTRACTION("-"),
    MULTIPLICATION("*"),
    DIVISION("/");

    fun describe(): String {
        return "Operation: $name ($symbol)"
    }
}
