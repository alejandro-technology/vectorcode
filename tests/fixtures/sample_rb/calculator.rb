module Calculator
  class Engine
    attr_reader :value, :name

    def initialize(initial = 0, name = "default")
      @value = initial
      @name = name
    end

    def add(n)
      @value += n
      puts "Adding #{n}, result: #{@value}"
      self
    end

    def subtract(n)
      @value -= n
      self
    end

    def multiply(n)
      @value *= n
      self
    end

    def result
      @value
    end

    def reset
      @value = 0
      self
    end
  end

  class AdvancedEngine < Engine
    attr_reader :history

    def initialize(initial = 0, name = "advanced")
      super(initial, name)
      @history = []
    end

    def add(n)
      @history << { operation: :add, value: n, result: @value + n }
      super(n)
    end

    def subtract(n)
      @history << { operation: :subtract, value: n, result: @value - n }
      super(n)
    end

    def last_operation
      @history.last
    end
  end

  module Helpers
    def self.format_result(value)
      "Result: #{value}"
    end

    def self.validate_input(input)
      raise ArgumentError, "Input must be numeric" unless input.is_a?(Numeric)
      true
    end
  end
end
