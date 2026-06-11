using System;
using System.Collections.Generic;

namespace Calculator.App
{
    public interface ICalculator
    {
        int Add(int n);
        int GetValue();
    }

    public class Calculator : ICalculator
    {
        private int _value;
        private string _name;

        public Calculator(int initial, string name)
        {
            _value = initial;
            _name = name;
        }

        public int Add(int n)
        {
            _value += n;
            Console.WriteLine($"Adding {n}, result: {_value}");
            return _value;
        }

        public int Subtract(int n)
        {
            _value -= n;
            return _value;
        }

        public int GetValue()
        {
            return _value;
        }

        public string GetName()
        {
            return _name;
        }
    }

    public enum OperationType
    {
        Addition,
        Subtraction,
        Multiplication,
        Division
    }
}
