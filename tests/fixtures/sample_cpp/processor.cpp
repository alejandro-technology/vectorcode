#include <string>
#include <vector>
#include <memory>
#include <iostream>

namespace data {

class DataProcessor {
public:
    DataProcessor(const std::string& name)
        : name_(name), processed_count_(0) {}

    void process(const std::vector<int>& data) {
        for (int item : data) {
            results_.push_back(item * 2);
            processed_count_++;
        }
        std::cout << "Processed " << processed_count_ << " items" << std::endl;
    }

    const std::vector<int>& results() const {
        return results_;
    }

    const std::string& name() const {
        return name_;
    }

    int processed_count() const {
        return processed_count_;
    }

private:
    std::string name_;
    std::vector<int> results_;
    int processed_count_;
};

struct DataConfig {
    std::string input_path;
    std::string output_path;
    int batch_size;
    bool verbose;
};

} // namespace data

namespace utils {

class Logger {
public:
    Logger(const std::string& component) : component_(component) {}

    void info(const std::string& message) {
        std::cout << "[INFO] " << component_ << ": " << message << std::endl;
    }

    void error(const std::string& message) {
        std::cerr << "[ERROR] " << component_ << ": " << message << std::endl;
    }

private:
    std::string component_;
};

} // namespace utils
