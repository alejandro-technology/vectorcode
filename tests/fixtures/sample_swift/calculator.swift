import Foundation

protocol Calculatable {
    func add(_ n: Int) -> Int
    func subtract(_ n: Int) -> Int
    func result() -> Int
}

class Calculator: Calculatable {
    private var value: Int
    private let name: String

    init(initial: Int = 0, name: String = "default") {
        self.value = initial
        self.name = name
    }

    func add(_ n: Int) -> Int {
        value += n
        print("Adding \(n), result: \(value)")
        return value
    }

    func subtract(_ n: Int) -> Int {
        value -= n
        return value
    }

    func result() -> Int {
        return value
    }

    func reset() {
        value = 0
    }
}

enum Operation: String, CaseIterable {
    case addition = "Addition"
    case subtraction = "Subtraction"
    case multiplication = "Multiplication"
    case division = "Division"

    func description() -> String {
        return "Operation: \(rawValue)"
    }
}

struct CalculationResult {
    let operation: Operation
    let input: Int
    let output: Int
    let timestamp: Date

    func formatted() -> String {
        return "\(operation.rawValue): \(input) -> \(output)"
    }
}
